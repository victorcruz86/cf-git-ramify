use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::Path;

use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use similar::{capture_diff_slices, Algorithm, DiffOp};
use tempfile::TempDir;

use crate::commit::PartialFile;
use crate::diff;
use crate::git::{self, GitError};
use crate::snapshots::{self, SnapshotId};
use crate::storage;

pub const BEFORE_LINES_ROLLBACK: &str = "before rolling back lines";

// A contiguous change between two versions of a file, without context lines. Positions
// count lines from 0: `old_start` is the first affected line in the old version and
// `new_start` in the new one (if the range is empty, the point where it would enter).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    pub id: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
}

// What is chosen from a hunk: `None` is the whole hunk.
//
// `lines` numbers the hunk's changed lines in a single sequence: first the old ones
// (deletions), from 0 to `old_lines - 1`, and then the new ones (additions), from `old_lines` to
// `old_lines + new_lines - 1`. Choosing a deletion applies it (the line leaves the result); not
// choosing it keeps the line. Choosing an addition applies it (it enters); not choosing it, it does not enter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineSelection {
    pub hunk: String,
    #[serde(deserialize_with = "required")]
    pub lines: Option<Vec<u32>>,
}

// Without this, a missing `lines` would be read as `None`, the whole hunk. It must come (`null` or a list).
fn required<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Revalidated {
    pub kept: Vec<LineSelection>,
    pub lost: Vec<String>,
}

// The lines keep their terminator, also the last one if the file does not end in a newline.
fn split_lines(bytes: &[u8]) -> Vec<&[u8]> {
    bytes.split_inclusive(|byte| *byte == b'\n').collect()
}

struct Parts<'a> {
    old: Vec<&'a [u8]>,
    new: Vec<&'a [u8]>,
    hunks: Vec<Hunk>,
}

// The unchanged lines looked at on each side of the hunk to recognize it.
const CONTEXT_LINES: usize = 3;

fn feed(digest: &mut Sha256, lines: &[&[u8]]) {
    digest.update((lines.len() as u64).to_le_bytes());
    for line in lines {
        digest.update((line.len() as u64).to_le_bytes());
        digest.update(line);
    }
}

// What identifies a hunk without depending on its position: its lines and the three from the old
// version on each side (with lengths in front, so that no two inputs have the same bytes).
// The context comes from the old version, which does not change when Devin edits another part.
fn hunk_key(before: &[&[u8]], old: &[&[u8]], new: &[&[u8]], after: &[&[u8]]) -> [u8; 32] {
    let mut digest = Sha256::new();
    for part in [before, old, new, after] {
        feed(&mut digest, part);
    }
    digest.finalize().into()
}

// If two hunks match even in the context they are told apart by their order and by how many there are: when
// one is removed, the one that remains does not inherit the id of either.
fn hunk_id(key: &[u8; 32], occurrence: u32, count: u32) -> String {
    let mut digest = Sha256::new();
    digest.update(key);
    digest.update(occurrence.to_le_bytes());
    digest.update(count.to_le_bytes());
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn analyze<'a>(base: &'a [u8], current: &'a [u8]) -> Parts<'a> {
    let old = split_lines(base);
    let new = split_lines(current);
    let mut ranges: Vec<(usize, usize, usize, usize)> = Vec::new();
    for op in capture_diff_slices(Algorithm::Myers, &old, &new) {
        let (old_start, old_len, new_start, new_len) = match op {
            DiffOp::Equal { .. } => continue,
            DiffOp::Delete {
                old_index,
                old_len,
                new_index,
            } => (old_index, old_len, new_index, 0),
            DiffOp::Insert {
                old_index,
                new_index,
                new_len,
            } => (old_index, 0, new_index, new_len),
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => (old_index, old_len, new_index, new_len),
        };
        match ranges.last_mut() {
            Some(last) if last.0 + last.1 == old_start && last.2 + last.3 == new_start => {
                last.1 += old_len;
                last.3 += new_len;
            }
            _ => ranges.push((old_start, old_len, new_start, new_len)),
        }
    }

    let keys: Vec<[u8; 32]> = ranges
        .iter()
        .map(|&(old_start, old_len, new_start, new_len)| {
            let old_end = old_start + old_len;
            hunk_key(
                &old[old_start.saturating_sub(CONTEXT_LINES)..old_start],
                &old[old_start..old_end],
                &new[new_start..new_start + new_len],
                &old[old_end..old.len().min(old_end + CONTEXT_LINES)],
            )
        })
        .collect();
    let mut counts: HashMap<[u8; 32], u32> = HashMap::new();
    for key in &keys {
        *counts.entry(*key).or_insert(0) += 1;
    }
    let mut seen: HashMap<[u8; 32], u32> = HashMap::new();
    let hunks = ranges
        .into_iter()
        .zip(&keys)
        .map(|((old_start, old_len, new_start, new_len), key)| {
            let occurrence = seen.entry(*key).or_insert(0);
            let id = hunk_id(key, *occurrence, counts[key]);
            *occurrence += 1;
            Hunk {
                id,
                old_start: old_start as u32,
                old_lines: old_len as u32,
                new_start: new_start as u32,
                new_lines: new_len as u32,
            }
        })
        .collect();
    Parts { old, new, hunks }
}

pub fn compute(base: &[u8], current: &[u8]) -> Vec<Hunk> {
    analyze(base, current).hunks
}

enum Picked {
    All,
    Lines(BTreeSet<u32>),
}

impl Picked {
    fn has(&self, index: u32) -> bool {
        match self {
            Picked::All => true,
            Picked::Lines(lines) => lines.contains(&index),
        }
    }
}

// Several entries of the same hunk are merged; the whole hunk wins over single lines.
fn pick(selection: &[LineSelection]) -> HashMap<&str, Picked> {
    let mut picked: HashMap<&str, Picked> = HashMap::new();
    for entry in selection {
        let slot = picked
            .entry(entry.hunk.as_str())
            .or_insert_with(|| Picked::Lines(BTreeSet::new()));
        match (&entry.lines, slot) {
            (None, slot) => *slot = Picked::All,
            (Some(lines), Picked::Lines(set)) => set.extend(lines),
            (Some(_), Picked::All) => {}
        }
    }
    picked
}

// A file without a final newline can only have its last line unterminated. If when choosing lines
// another one follows it, the file's line ending is added so they do not stick together.
struct Output {
    bytes: Vec<u8>,
    open: bool,
    ending: &'static [u8],
}

impl Output {
    fn new(old: &[&[u8]], new: &[&[u8]]) -> Self {
        let first = new.iter().chain(old).find(|line| line.ends_with(b"\n"));
        let crlf = first.is_some_and(|line| line.ends_with(b"\r\n"));
        Output {
            bytes: Vec::new(),
            open: false,
            ending: if crlf { b"\r\n" } else { b"\n" },
        }
    }

    fn push(&mut self, line: &[u8]) {
        if self.open {
            self.bytes.extend_from_slice(self.ending);
        }
        self.bytes.extend_from_slice(line);
        self.open = !line.ends_with(b"\n");
    }
}

// Starts from `base` and moves toward `current`. With `invert` it starts from `current` and goes toward `base`:
// what one applies, the other undoes.
fn assemble(base: &[u8], current: &[u8], selection: &[LineSelection], invert: bool) -> Vec<u8> {
    let parts = analyze(base, current);
    let picked = pick(selection);
    let mut out = Output::new(&parts.old, &parts.new);
    let mut position = 0;
    for hunk in &parts.hunks {
        let (old_start, old_end) = (
            hunk.old_start as usize,
            (hunk.old_start + hunk.old_lines) as usize,
        );
        let (new_start, new_end) = (
            hunk.new_start as usize,
            (hunk.new_start + hunk.new_lines) as usize,
        );
        for line in &parts.old[position..old_start] {
            out.push(line);
        }
        let chosen = picked.get(hunk.id.as_str());
        let selected = |index: u32| chosen.is_some_and(|chosen| chosen.has(index));
        // Old and new line of the same position go together: this way, inside a replacement of
        // several lines, choosing one pair does not reorder the rest. What is left over goes after, in order.
        let (olds, news) = (
            &parts.old[old_start..old_end],
            &parts.new[new_start..new_end],
        );
        for position in 0..olds.len().max(news.len()) {
            if let Some(line) = olds.get(position) {
                if selected(position as u32) == invert {
                    out.push(line);
                }
            }
            if let Some(line) = news.get(position) {
                if selected(hunk.old_lines + position as u32) != invert {
                    out.push(line);
                }
            }
        }
        position = old_end;
    }
    for line in &parts.old[position..] {
        out.push(line);
    }
    out.bytes
}

// The content that would enter a commit: `base` with the chosen changes applied.
pub fn partial_content(base: &[u8], current: &[u8], selection: &[LineSelection]) -> Vec<u8> {
    assemble(base, current, selection, false)
}

// The content that remains when undoing the chosen changes in `current`.
pub fn revert_content(base: &[u8], current: &[u8], selection: &[LineSelection]) -> Vec<u8> {
    assemble(base, current, selection, true)
}

// Whether some line of `content` does not remain, in its place relative to the others, in any of the
// `kept` versions: a line counts only if the diff with that version leaves it equal, because an
// order that is only here could not be recovered from them either.
pub fn loses_lines(content: &[u8], kept: &[&[u8]]) -> bool {
    let lines = split_lines(content);
    let mut safe = vec![false; lines.len()];
    for version in kept {
        let other = split_lines(version);
        for op in capture_diff_slices(Algorithm::Myers, &lines, &other) {
            if let DiffOp::Equal { old_index, len, .. } = op {
                safe[old_index..old_index + len].fill(true);
            }
        }
    }
    safe.contains(&false)
}

// Separates the chosen changes that still exist from the hunks that are gone (they changed).
pub fn revalidate(selection: &[LineSelection], hunks: &[Hunk]) -> Revalidated {
    let known: HashSet<&str> = hunks.iter().map(|hunk| hunk.id.as_str()).collect();
    let mut result = Revalidated {
        kept: Vec::new(),
        lost: Vec::new(),
    };
    for entry in selection {
        if known.contains(entry.hunk.as_str()) {
            result.kept.push(entry.clone());
        } else if !result.lost.contains(&entry.hunk) {
            result.lost.push(entry.hunk.clone());
        }
    }
    result
}

#[derive(Debug)]
pub enum HunkError {
    OutsideRepository(String),
    // Binary, too large or not a regular file: it is treated as a single piece.
    NotSplittable(String),
    // Chosen hunks of `path` that no longer exist because the file changed.
    Lost { path: String, hunks: Vec<String> },
    OutOfRange(String),
    NothingSelected,
    // The result could not be converted to the disk form (for example, a filter is missing).
    Convert { path: String, detail: String },
    // The file changed on disk while the operation was being prepared.
    Changed(String),
    // Saving the displayed text would not leave the same bytes the file has.
    NotEditable(String),
    // With a conflict: rolling back lines would break its markers, and touching the stage, its stages.
    Unmerged(String),
    // In the place of the file there is now a folder, or the file occupies its folder's place.
    Displaced(String),
    // HEAD, the stage or the disk is missing, or one of them is not text that can be edited.
    NoThreeSides(String),
    Git(GitError),
    Io(io::Error),
    Snapshot(io::Error),
}

impl fmt::Display for HunkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutsideRepository(path) => {
                write!(f, "The path \"{path}\" is not valid inside the repository")
            }
            Self::NotSplittable(path) => write!(
                f,
                "\"{path}\" is binary, too large or not a regular file: it cannot be selected by lines"
            ),
            Self::Lost { .. } => write!(
                f,
                "Some of the chosen changes no longer exist because the file has changed"
            ),
            Self::OutOfRange(_) => write!(f, "The selection points to a line that does not exist"),
            Self::NothingSelected => write!(f, "No line is selected"),
            Self::Convert { path, detail } => write!(
                f,
                "Could not prepare \"{path}\" to write it to disk: {detail}"
            ),
            Self::Changed(path) => write!(
                f,
                "\"{path}\" changed while the operation was being prepared; nothing has been written"
            ),
            Self::NotEditable(path) => write!(f, "\"{path}\" cannot be edited from here"),
            Self::Unmerged(path) => write!(
                f,
                "\"{path}\" has unresolved conflicts: resolve them before continuing"
            ),
            Self::Displaced(path) => write!(
                f,
                "There is now something else on disk in the place of \"{path}\": reverting would delete it, so nothing has been touched"
            ),
            Self::NoThreeSides(path) => write!(
                f,
                "\"{path}\" does not have HEAD, stage and disk as editable text: they are compared two at a time"
            ),
            Self::Git(error) => write!(f, "{}", crate::commands::ApiError::from(error.clone()).message),
            Self::Io(error) => write!(f, "{error}"),
            Self::Snapshot(error) => write!(f, "Could not save the backup copy: {error}"),
        }
    }
}

impl std::error::Error for HunkError {}

impl From<GitError> for HunkError {
    fn from(error: GitError) -> Self {
        Self::Git(error)
    }
}

impl From<io::Error> for HunkError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub struct OnDisk {
    pub bytes: Vec<u8>,
    pub permissions: fs::Permissions,
}

// `None` if there is nothing to split: it does not exist, it is not a regular file (a link is one piece)
// or it exceeds the `diff` limit.
pub fn read_current(repo: &Path, path: &str) -> Result<Option<OnDisk>, HunkError> {
    let full = repo.join(path);
    let meta = match fs::symlink_metadata(&full) {
        Ok(meta) => meta,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(None)
        }
        Err(error) => return Err(error.into()),
    };
    if !meta.is_file() || meta.len() > diff::MAX_DIFF_BYTES {
        return Ok(None);
    }
    // With a cap, in case the file grows between checking its size and reading it.
    let mut bytes = Vec::new();
    fs::File::open(&full)?
        .take(diff::MAX_DIFF_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > diff::MAX_DIFF_BYTES {
        return Ok(None);
    }
    Ok(Some(OnDisk {
        bytes,
        permissions: meta.permissions(),
    }))
}

// The content of a blob; `None` if it exceeds the `diff` limit.
fn read_blob(repo: &Path, oid: &str) -> Result<Option<Vec<u8>>, HunkError> {
    diff::check_hash(oid)?;
    let size = git::run(repo, &["cat-file", "-s", oid])?;
    let size: u64 = String::from_utf8_lossy(&size)
        .trim()
        .parse()
        .map_err(|_| io::Error::other(format!("invalid size for object {oid}")))?;
    if size > diff::MAX_DIFF_BYTES {
        return Ok(None);
    }
    Ok(Some(git::run(repo, &["cat-file", "blob", oid])?))
}

// A record of `ls-tree -z` or `ls-files -s -z`: mode, hash and (depending on the command) stage or type,
// separated by spaces and followed by a tab and the path.
fn entry_fields(output: &[u8]) -> Option<Vec<String>> {
    let record = output.split(|byte| *byte == 0).next()?;
    let head = record.split(|byte| *byte == b'\t').next()?;
    let fields: Vec<String> = String::from_utf8_lossy(head)
        .split_whitespace()
        .map(str::to_string)
        .collect();
    (fields.len() == 3).then_some(fields)
}

const REGULAR_MODES: [&str; 2] = ["100644", "100755"];
const GITLINK_MODE: &str = "160000";

// HEAD's blob, empty if the file is new or there are no commits yet; `None` if it cannot be split
// into lines: a link, a folder or a submodule in HEAD, or a blob that exceeds the limit.
pub fn read_head(repo: &Path, path: &str) -> Result<Option<Vec<u8>>, HunkError> {
    if !git::head_exists(repo)? {
        return Ok(Some(Vec::new()));
    }
    let args = git::exact(&["--literal-pathspecs", "ls-tree", "-z", "HEAD", "--", path]);
    let listed = git::run(repo, &args)?;
    if listed.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let Some(entry) = entry_fields(&listed) else {
        return Err(io::Error::other(format!("Invalid output from git ls-tree for {path}")).into());
    };
    if !REGULAR_MODES.contains(&entry[0].as_str()) {
        return Ok(None);
    }
    read_blob(repo, &entry[2])
}

// The blob the index has for `path`, empty if it has none; `None` if it cannot be split
// into lines, as in `read_head`.
pub fn read_index(repo: &Path, path: &str) -> Result<Option<Vec<u8>>, HunkError> {
    let args = git::exact(&["--literal-pathspecs", "ls-files", "-s", "-z", "--", path]);
    let listed = git::run(repo, &args)?;
    if listed.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let Some(entry) = entry_fields(&listed) else {
        return Err(
            io::Error::other(format!("Invalid output from git ls-files for {path}")).into(),
        );
    };
    if entry[2] != "0" {
        return Err(HunkError::Unmerged(path.to_string()));
    }
    if !REGULAR_MODES.contains(&entry[0].as_str()) {
        return Ok(None);
    }
    read_blob(repo, &entry[1])
}

// What `git add` would store of this file: with `autocrlf`, `eol`, filters and `ident` already
// applied. Git is asked on a copy of the index, which is deleted at the end: the real index is not
// touched. It is the form in which HEAD stores the file, and therefore the one its lines are
// compared against.
pub fn stored_form(repo: &Path, path: &str) -> Result<Option<Vec<u8>>, HunkError> {
    let Some(entry) = stored_entry(repo, path)? else {
        return Err(io::Error::other(format!("git did not store \"{path}\"")).into());
    };
    read_blob(repo, &entry[1])
}

// Mode, hash and stage with which `git add` would store `path`.
fn stored_entry(repo: &Path, path: &str) -> Result<Option<Vec<String>>, HunkError> {
    let index = git::git_path(repo, "index")?;
    let temp = TempDir::new()?;
    let copy = temp.path().join("index");
    match fs::copy(&index, &copy) {
        Ok(_) => {}
        // No index yet (newly created repository): git starts an empty one.
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let envs = [("GIT_INDEX_FILE", copy.as_path())];
    let add = git::exact(&["--literal-pathspecs", "add", "--force", "--", path]);
    git::run_with_env(repo, &add, &envs)?;
    let list = git::exact(&["--literal-pathspecs", "ls-files", "-s", "-z", "--", path]);
    Ok(entry_fields(&git::run_with_env(repo, &list, &envs)?))
}

pub struct Sides {
    // HEAD and the file, both in the form git stores.
    pub base: Vec<u8>,
    pub stored: Vec<u8>,
    // The file's bytes and permissions as they are on disk, from which `stored` comes.
    pub disk: OnDisk,
}

// `None`: the file is not split into hunks and the caller treats it as a single piece.
pub fn check_path(repo: &Path, path: &str) -> Result<(), HunkError> {
    snapshots::check_path(repo, path).map_err(|error| match error.kind() {
        io::ErrorKind::InvalidInput => HunkError::OutsideRepository(path.to_string()),
        _ => HunkError::Io(error),
    })
}

pub fn read_sides(repo: &Path, path: &str) -> Result<Option<Sides>, HunkError> {
    check_path(repo, path)?;
    // It is read before asking for the stored form: if the file changes between the two, the
    // check before writing detects it.
    let Some(disk) = read_current(repo, path)? else {
        return Ok(None);
    };
    let Some(base) = read_head(repo, path)? else {
        return Ok(None);
    };
    if diff::looks_binary(&base) || diff::looks_binary(&disk.bytes) {
        return Ok(None);
    }
    let Some(stored) = stored_form(repo, path)? else {
        return Ok(None);
    };
    if diff::looks_binary(&stored) {
        return Ok(None);
    }
    Ok(Some(Sides { base, stored, disk }))
}

// HEAD's blob to compare against; empty if it cannot be split into lines.
pub fn head_bytes(repo: &Path, path: &str) -> Result<Vec<u8>, HunkError> {
    Ok(read_head(repo, path)?.unwrap_or_default())
}

// The commit the submodule has now, if `path` is one.
pub fn stored_gitlink(repo: &Path, path: &str) -> Result<Option<String>, HunkError> {
    let list = git::exact(&["--literal-pathspecs", "ls-files", "-s", "-z", "--", path]);
    let tracked = entry_fields(&git::run(repo, &list)?);
    if !tracked.is_some_and(|entry| entry[0] == GITLINK_MODE) {
        return Ok(None);
    }
    let Some(entry) = stored_entry(repo, path)? else {
        return Ok(None);
    };
    Ok((entry[0] == GITLINK_MODE).then(|| entry[1].clone()))
}

// The commit the index has for `path`, if it is a submodule.
pub fn index_gitlink(repo: &Path, path: &str) -> Result<Option<String>, HunkError> {
    let list = git::exact(&["--literal-pathspecs", "ls-files", "-s", "-z", "--", path]);
    let entry = entry_fields(&git::run(repo, &list)?);
    Ok(entry
        .filter(|entry| entry[0] == GITLINK_MODE)
        .map(|entry| entry[1].clone()))
}

// The hunks of the file on disk against HEAD. No hunks (empty list) if it is binary, very large
// or not a regular file: it is treated as a whole.
pub fn file_hunks(repo: &Path, path: &str) -> Result<Vec<Hunk>, HunkError> {
    Ok(read_sides(repo, path)?
        .map(|sides| compute(&sides.base, &sides.stored))
        .unwrap_or_default())
}

// That the chosen hunks exist and the lines are inside their hunk.
pub fn check_selection(
    path: &str,
    selection: &[LineSelection],
    hunks: &[Hunk],
) -> Result<(), HunkError> {
    let revalidated = revalidate(selection, hunks);
    if !revalidated.lost.is_empty() {
        return Err(HunkError::Lost {
            path: path.to_string(),
            hunks: revalidated.lost,
        });
    }
    for entry in selection {
        let Some(hunk) = hunks.iter().find(|hunk| hunk.id == entry.hunk) else {
            continue;
        };
        let total = hunk.old_lines + hunk.new_lines;
        if entry.lines.iter().flatten().any(|line| *line >= total) {
            return Err(HunkError::OutOfRange(entry.hunk.clone()));
        }
    }
    Ok(())
}

pub fn selects_nothing(selection: &[LineSelection]) -> bool {
    selection
        .iter()
        .all(|entry| entry.lines.as_ref().is_some_and(Vec::is_empty))
}

// The file as it would enter a commit with that selection, computed with what is on disk
// now. `content` is in the form git stores (what `git add` of the file would store), which
// is what `commit` writes without filtering. The bytes do not go through text or JSON.
pub fn partial_file(
    repo: &Path,
    path: &str,
    selection: &[LineSelection],
) -> Result<PartialFile, HunkError> {
    let sides =
        read_sides(repo, path)?.ok_or_else(|| HunkError::NotSplittable(path.to_string()))?;
    check_selection(path, selection, &compute(&sides.base, &sides.stored))?;
    // Nothing chosen is not "the file as it was": it would be a commit with the file empty.
    if selects_nothing(selection) {
        return Err(HunkError::NothingSelected);
    }
    Ok(PartialFile {
        path: path.to_string(),
        content: partial_content(&sides.base, &sides.stored, selection),
    })
}

// Vcs.RollbackChangedLines: undoes the chosen lines in the file on disk. The result is
// computed in the form git stores and converted to the disk form with the same git filters
// (`cat-file --filters`); if that fails nothing is written. Before writing it saves a snapshot, and
// writes only if the file is still the same as when it was read. The caller must hold the repository's
// operations lock.
pub fn rollback_lines(
    repo: &Path,
    dir: &Path,
    path: &str,
    selection: &[LineSelection],
) -> Result<SnapshotId, HunkError> {
    rollback_lines_with(repo, dir, path, selection, || {})
}

// The result in the disk form: it is stored as a blob without filters and git is asked for the
// content of that blob as a checkout of `path` would leave it.
fn disk_form(repo: &Path, path: &str, stored: &[u8]) -> Result<Vec<u8>, HunkError> {
    let hash = git::run_with_input(
        repo,
        &["hash-object", "-w", "--stdin", "--no-filters"],
        stored,
    )?;
    let oid = String::from_utf8_lossy(&hash).trim().to_string();
    diff::check_hash(&oid)?;
    let path_option = format!("--path={path}");
    let args = git::exact(&["cat-file", "--filters", &path_option, &oid]);
    git::run(repo, &args).map_err(|error| match error {
        GitError::Failed { stderr, .. } => HunkError::Convert {
            path: path.to_string(),
            detail: stderr.trim().to_string(),
        },
        other => other.into(),
    })
}

// `after_snapshot` runs between the snapshot and the final check: the tests edit the file
// there, which is where Devin could do it.
fn rollback_lines_with(
    repo: &Path,
    dir: &Path,
    path: &str,
    selection: &[LineSelection],
    after_snapshot: impl FnOnce(),
) -> Result<SnapshotId, HunkError> {
    let sides =
        read_sides(repo, path)?.ok_or_else(|| HunkError::NotSplittable(path.to_string()))?;
    let unmerged = git::exact(&["--literal-pathspecs", "ls-files", "-u", "-z", "--", path]);
    if !git::run(repo, &unmerged)?.is_empty() {
        return Err(HunkError::Unmerged(path.to_string()));
    }
    check_selection(path, selection, &compute(&sides.base, &sides.stored))?;
    if selects_nothing(selection) {
        return Err(HunkError::NothingSelected);
    }
    let reverted = revert_content(&sides.base, &sides.stored, selection);
    if reverted == sides.stored {
        return Err(HunkError::NothingSelected);
    }
    let on_disk = disk_form(repo, path, &reverted)?;

    let snapshot = snapshots::create(dir, repo, &[path.to_string()], BEFORE_LINES_ROLLBACK)
        .map_err(HunkError::Snapshot)?;
    after_snapshot();

    let now = read_current(repo, path)?;
    let unchanged = now
        .as_ref()
        .is_some_and(|now| now.bytes == sides.disk.bytes);
    let Some(now) = now.filter(|_| unchanged) else {
        return Err(HunkError::Changed(path.to_string()));
    };
    storage::write_atomic_with(&repo.join(path), &on_disk, Some(now.permissions))?;
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rolling_back_lines_in_a_conflicted_file_is_rejected() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"base\n", "base");
        repo.git(&["checkout", "-q", "-b", "other"]);
        repo.commit("a.txt", b"other\n", "on other");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("a.txt", b"main\n", "on main");
        let merge = crate::test_support::git_command()
            .current_dir(repo.path())
            .args(["merge", "-q", "other"])
            .output()
            .unwrap();
        assert!(!merge.status.success());
        let before = fs::read(repo.path().join("a.txt")).unwrap();
        let hunks = file_hunks(repo.path(), "a.txt").unwrap();
        let dir = TempDir::new().unwrap();

        let result = rollback_lines(repo.path(), dir.path(), "a.txt", &[whole(&hunks[0])]);

        assert!(matches!(result, Err(HunkError::Unmerged(_))));
        assert_eq!(fs::read(repo.path().join("a.txt")).unwrap(), before);
    }

    #[test]
    fn rolling_back_lines_of_another_file_during_a_conflict_works() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"base\n", "base");
        repo.commit("b.txt", b"one\n", "b");
        repo.git(&["checkout", "-q", "-b", "other"]);
        repo.commit("a.txt", b"other\n", "on other");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("a.txt", b"main\n", "on main");
        let _ = crate::test_support::git_command()
            .current_dir(repo.path())
            .args(["merge", "-q", "other"])
            .output()
            .unwrap();
        fs::write(repo.path().join("b.txt"), b"two\n").unwrap();
        let hunks = file_hunks(repo.path(), "b.txt").unwrap();
        let dir = TempDir::new().unwrap();

        rollback_lines(repo.path(), dir.path(), "b.txt", &[whole(&hunks[0])]).unwrap();

        assert_eq!(fs::read(repo.path().join("b.txt")).unwrap(), b"one\n");
    }

    fn whole(hunk: &Hunk) -> LineSelection {
        LineSelection {
            hunk: hunk.id.clone(),
            lines: None,
        }
    }

    fn lines(hunk: &Hunk, lines: &[u32]) -> LineSelection {
        LineSelection {
            hunk: hunk.id.clone(),
            lines: Some(lines.to_vec()),
        }
    }

    const NINE: &[u8] = b"1\n2\n3\n4\n5\n6\n7\n8\n9\n";
    const NINE_EDITED: &[u8] = b"1\ntwo\n3\n4\n5\n6\n7\neight\n9\n";

    #[test]
    fn hunks_with_their_positions() {
        let hunks = compute(NINE, NINE_EDITED);

        let ranges: Vec<_> = hunks
            .iter()
            .map(|h| (h.old_start, h.old_lines, h.new_start, h.new_lines))
            .collect();
        assert_eq!(ranges, [(1, 1, 1, 1), (7, 1, 7, 1)]);
    }

    #[test]
    fn identical_files_have_no_hunks() {
        assert!(compute(NINE, NINE).is_empty());
        assert!(compute(b"", b"").is_empty());
    }

    #[test]
    fn new_file_and_empty_file() {
        let added = compute(b"", b"a\nb\n");
        assert_eq!(added.len(), 1);
        assert_eq!((added[0].old_lines, added[0].new_lines), (0, 2));

        let removed = compute(b"a\nb\n", b"");
        assert_eq!(removed.len(), 1);
        assert_eq!((removed[0].old_lines, removed[0].new_lines), (2, 0));
    }

    #[test]
    fn the_id_does_not_depend_on_the_position() {
        let moved = compute(
            b"new\n1\n2\n3\n4\n5\n6\n7\n8\n9\n",
            b"new\n1\ntwo\n3\n4\n5\n6\n7\neight\n9\n",
        );
        let hunks = compute(NINE, NINE_EDITED);

        // The second change has three context lines on one side and one on the other, the same in both.
        assert_ne!(moved[1].old_start, hunks[1].old_start);
        assert_eq!(moved[1].id, hunks[1].id);
        assert_ne!(hunks[0].id, hunks[1].id);
    }

    fn distinct(hunks: &[Hunk]) -> usize {
        let mut ids: Vec<_> = hunks.iter().map(|h| h.id.clone()).collect();
        ids.sort();
        ids.dedup();
        ids.len()
    }

    // Two changes with the same content and the same three lines on each side.
    const TWICE: &[u8] = b"1\n2\n3\n4\n5\n6\n1\n2\n3\n4\n5\n6\n";
    const TWICE_EDITED: &[u8] = b"1\n2\n3\nY\n4\n5\n6\n1\n2\n3\nY\n4\n5\n6\n";
    const TWICE_ONE_LEFT: &[u8] = b"1\n2\n3\n4\n5\n6\n1\n2\n3\nY\n4\n5\n6\n";

    #[test]
    fn equal_hunks_have_distinct_ids() {
        let hunks = compute(TWICE, TWICE_EDITED);

        assert_eq!(hunks.len(), 2);
        assert_eq!(distinct(&hunks), 2);
        // They are still the same when computed again.
        assert_eq!(compute(TWICE, TWICE_EDITED), hunks);
    }

    #[test]
    fn hunks_with_the_same_change_and_other_context_have_distinct_ids() {
        let base = b"a\nx\nb\nx\nc\nx\nd\n";
        let current = b"a\nx\ny\nb\nx\ny\nc\nx\ny\nd\n";

        let hunks = compute(base, current);

        assert_eq!(hunks.len(), 3);
        assert_eq!(distinct(&hunks), 3);
    }

    #[test]
    fn removing_an_equal_hunk_does_not_move_the_selection_to_the_next() {
        let base = b"a\nx\nb\nx\nc\nx\nd\n";
        let current = b"a\nx\ny\nb\nx\ny\nc\nx\ny\nd\n";
        let before = compute(base, current);
        let selection = [whole(&before[1])];
        // Devin removes the first change; the chosen one is still the one in the middle.
        let edited = b"a\nx\nb\nx\ny\nc\nx\ny\nd\n";

        let after = compute(base, edited);
        let result = revalidate(&selection, &after);

        assert_eq!(result.kept, selection);
        assert_eq!(
            partial_content(base, edited, &selection),
            b"a\nx\nb\nx\ny\nc\nx\nd\n"
        );
        assert_eq!(
            revert_content(base, edited, &selection),
            b"a\nx\nb\nx\nc\nx\ny\nd\n"
        );
    }

    #[test]
    fn with_equal_context_removing_one_loses_the_selections_of_both() {
        let before = compute(TWICE, TWICE_EDITED);
        let first = [whole(&before[0])];
        let second = [whole(&before[1])];

        let after = compute(TWICE, TWICE_ONE_LEFT);

        assert_eq!(after.len(), 1);
        for selection in [&first, &second] {
            let result = revalidate(selection, &after);
            assert!(result.kept.is_empty());
            assert_eq!(result.lost, [selection[0].hunk.clone()]);
        }
    }

    #[test]
    fn with_equal_context_both_stay_the_same_if_nothing_changes() {
        let before = compute(TWICE, TWICE_EDITED);
        let after = compute(TWICE, TWICE_EDITED);
        let selection = [whole(&before[1])];

        assert_eq!(revalidate(&selection, &after).kept, selection);
        assert_eq!(
            partial_content(TWICE, TWICE_EDITED, &selection),
            b"1\n2\n3\n4\n5\n6\n1\n2\n3\nY\n4\n5\n6\n"
        );
    }

    #[test]
    fn one_hunk_chosen_of_two() {
        let hunks = compute(NINE, NINE_EDITED);

        let first = partial_content(NINE, NINE_EDITED, &[whole(&hunks[0])]);
        let second = partial_content(NINE, NINE_EDITED, &[whole(&hunks[1])]);

        assert_eq!(first, b"1\ntwo\n3\n4\n5\n6\n7\n8\n9\n");
        assert_eq!(second, b"1\n2\n3\n4\n5\n6\n7\neight\n9\n");
    }

    #[test]
    fn none_chosen() {
        assert_eq!(partial_content(NINE, NINE_EDITED, &[]), NINE);
        let hunks = compute(NINE, NINE_EDITED);
        let none = [lines(&hunks[0], &[]), lines(&hunks[1], &[])];
        assert_eq!(partial_content(NINE, NINE_EDITED, &none), NINE);
    }

    #[test]
    fn all_chosen() {
        let hunks = compute(NINE, NINE_EDITED);
        let all: Vec<_> = hunks.iter().map(whole).collect();

        assert_eq!(partial_content(NINE, NINE_EDITED, &all), NINE_EDITED);
    }

    #[test]
    fn all_lines_chosen_give_the_whole_hunk() {
        let hunks = compute(NINE, NINE_EDITED);
        let all = [lines(&hunks[0], &[0, 1]), lines(&hunks[1], &[0, 1])];

        assert_eq!(partial_content(NINE, NINE_EDITED, &all), NINE_EDITED);
    }

    #[test]
    fn single_line_inside_a_hunk() {
        let base = b"a\nb\n";
        let current = b"a\nx\ny\nz\nb\n";
        let hunks = compute(base, current);
        assert_eq!(hunks.len(), 1);

        // Without old lines, indices 0, 1 and 2 are the three additions.
        let result = partial_content(base, current, &[lines(&hunks[0], &[1])]);

        assert_eq!(result, b"a\ny\nb\n");
    }

    #[test]
    fn replacement_with_only_the_deletion_or_only_the_addition() {
        let base = b"a\nold\nb\n";
        let current = b"a\nnew\nb\n";
        let hunks = compute(base, current);
        assert_eq!((hunks[0].old_lines, hunks[0].new_lines), (1, 1));

        let only_deletion = partial_content(base, current, &[lines(&hunks[0], &[0])]);
        let only_addition = partial_content(base, current, &[lines(&hunks[0], &[1])]);

        assert_eq!(only_deletion, b"a\nb\n");
        assert_eq!(only_addition, b"a\nold\nnew\nb\n");
    }

    #[test]
    fn several_replaced_lines_are_aligned_by_position() {
        let base = b"a\nb\nc\nz\n";
        let current = b"A\nB\nC\nz\n";
        let hunks = compute(base, current);
        assert_eq!(hunks.len(), 1);
        // 0..3 are a, b, c; 3..6 are A, B, C. b (1) and B (4) are chosen.
        let selection = [lines(&hunks[0], &[1, 4])];

        assert_eq!(partial_content(base, current, &selection), b"a\nB\nc\nz\n");
        assert_eq!(revert_content(base, current, &selection), b"A\nb\nC\nz\n");
    }

    #[test]
    fn uneven_replacement_leaves_the_excess_at_the_end() {
        let base = b"a\nb\nz\n";
        let current = b"A\nB\nC\nz\n";
        let hunks = compute(base, current);
        // 0..2 are a, b; 2..5 are A, B, C. Only C (4) is chosen.
        let selection = [lines(&hunks[0], &[4])];

        assert_eq!(partial_content(base, current, &selection), b"a\nb\nC\nz\n");
    }

    // Not checked against IntelliJ: the validation report notes it to compare by hand.
    #[test]
    fn shorter_replacement_with_one_line_chosen_leaves_it_among_the_old_ones() {
        let base = b"A\nB\nC\n";
        let current = b"X\nY\n";
        let hunks = compute(base, current);
        // 0..3 are A, B, C; 3..5 are X, Y. Only X (3) is chosen.
        let selection = [lines(&hunks[0], &[3])];

        assert_eq!(partial_content(base, current, &selection), b"A\nX\nB\nC\n");
    }

    #[test]
    fn no_final_newline() {
        // No newline in the new version.
        let base = b"a\nb\n";
        let current = b"a\nc";
        let hunks = compute(base, current);
        assert_eq!(partial_content(base, current, &[whole(&hunks[0])]), current);
        assert_eq!(partial_content(base, current, &[]), base);

        // No newline in either and the last line unchanged.
        let base = b"a\nb";
        let current = b"x\nb";
        let hunks = compute(base, current);
        assert_eq!(hunks.len(), 1);
        assert_eq!(partial_content(base, current, &[whole(&hunks[0])]), current);
        assert_eq!(partial_content(base, current, &[]), base);
    }

    #[test]
    fn new_line_after_one_without_final_newline() {
        let base = b"a\nb";
        let current = b"a\nb\nc";
        let hunks = compute(base, current);
        assert_eq!(hunks.len(), 1);

        // Only line `c`: the `b` without a newline needs one so it does not stick to it.
        let result = partial_content(base, current, &[lines(&hunks[0], &[2])]);

        assert_eq!(result, b"a\nb\nc");
        assert_eq!(partial_content(base, current, &[whole(&hunks[0])]), current);
    }

    #[test]
    fn crlf() {
        let base = b"a\r\nb\r\nc\r\nd\r\n";
        let current = b"a\r\nB\r\nc\r\nD\r\n";
        let hunks = compute(base, current);

        let first = partial_content(base, current, &[whole(&hunks[0])]);
        let all: Vec<_> = hunks.iter().map(whole).collect();

        assert_eq!(first, b"a\r\nB\r\nc\r\nd\r\n");
        assert_eq!(partial_content(base, current, &all), current);
        assert_eq!(partial_content(base, current, &[]), base);
    }

    #[test]
    fn crlf_with_new_line_after_one_without_final_newline() {
        let base = b"a\r\nb";
        let current = b"a\r\nb\r\nc";
        let hunks = compute(base, current);

        let result = partial_content(base, current, &[lines(&hunks[0], &[2])]);

        assert_eq!(result, b"a\r\nb\r\nc");
    }

    #[test]
    fn not_utf8() {
        let base = b"a\n\xff\xfe\nc\n\x80\n";
        let current = b"a\n\xff\xff\nc\n\x81\n";
        let hunks = compute(base, current);
        assert_eq!(hunks.len(), 2);

        let first = partial_content(base, current, &[whole(&hunks[0])]);
        let all: Vec<_> = hunks.iter().map(whole).collect();

        assert_eq!(first, b"a\n\xff\xff\nc\n\x80\n");
        assert_eq!(partial_content(base, current, &all), current);
        assert_eq!(partial_content(base, current, &[]), base);
    }

    #[test]
    fn repeated_selection_is_merged_and_the_unknown_one_is_ignored() {
        let hunks = compute(NINE, NINE_EDITED);
        let selection = [
            lines(&hunks[0], &[0]),
            lines(&hunks[0], &[1]),
            LineSelection {
                hunk: "does not exist".to_string(),
                lines: None,
            },
        ];

        let result = partial_content(NINE, NINE_EDITED, &selection);

        assert_eq!(result, b"1\ntwo\n3\n4\n5\n6\n7\n8\n9\n");
    }

    #[test]
    fn rollback_of_a_hunk() {
        let hunks = compute(NINE, NINE_EDITED);

        let result = revert_content(NINE, NINE_EDITED, &[whole(&hunks[1])]);

        assert_eq!(result, b"1\ntwo\n3\n4\n5\n6\n7\n8\n9\n");
    }

    #[test]
    fn rollback_with_nothing_or_with_everything() {
        let hunks = compute(NINE, NINE_EDITED);
        let all: Vec<_> = hunks.iter().map(whole).collect();

        assert_eq!(revert_content(NINE, NINE_EDITED, &[]), NINE_EDITED);
        assert_eq!(revert_content(NINE, NINE_EDITED, &all), NINE);
    }

    #[test]
    fn rollback_of_single_lines() {
        let base = b"a\nold\nb\n";
        let current = b"a\nnew\nb\n";
        let hunks = compute(base, current);

        // Only the deletion: the old line comes back and the new one stays.
        let deletion = revert_content(base, current, &[lines(&hunks[0], &[0])]);
        // Only the addition: the new one is removed and the old one does not come back.
        let addition = revert_content(base, current, &[lines(&hunks[0], &[1])]);

        assert_eq!(deletion, b"a\nold\nnew\nb\n");
        assert_eq!(addition, b"a\nb\n");
    }

    #[test]
    fn rollback_keeps_bytes_and_endings() {
        let base = b"a\r\n\xff\r\nz";
        let current = b"a\r\n\xfe\r\nz";
        let hunks = compute(base, current);

        assert_eq!(revert_content(base, current, &[whole(&hunks[0])]), base);
        assert_eq!(revert_content(base, current, &[]), current);
    }

    #[test]
    fn selections_survive_another_change() {
        let before = compute(NINE, NINE_EDITED);
        let selection = [whole(&before[1])];
        // Devin changes another part of the file.
        let edited = b"1\ntwo\n3\n4\nnew\n6\n7\neight\n9\n";

        let after = compute(NINE, edited);
        let result = revalidate(&selection, &after);

        assert_eq!(after.len(), 3);
        assert_eq!(result.kept, selection);
        assert!(result.lost.is_empty());
    }

    #[test]
    fn lost_selection_is_reported() {
        let before = compute(NINE, NINE_EDITED);
        let selection = [whole(&before[0]), lines(&before[1], &[0])];
        let edited = b"1\ntwo\n3\n4\n5\n6\n7\neighty\n9\n";

        let after = compute(NINE, edited);
        let result = revalidate(&selection, &after);

        assert_eq!(result.kept, [selection[0].clone()]);
        assert_eq!(result.lost, [before[1].id.clone()]);
    }

    // Small random files, with repeated lines, stray bytes and no final newline.
    fn random_file(seed: &mut u64) -> Vec<u8> {
        let mut next = |limit: u64| {
            *seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (*seed >> 33) % limit
        };
        let mut bytes = Vec::new();
        for _ in 0..next(8) {
            bytes.extend_from_slice([&b"a"[..], b"b", b"\xff", b"", b"cc"][next(5) as usize]);
            bytes.extend_from_slice([&b"\n"[..], b"\r\n"][next(2) as usize]);
        }
        if next(3) == 0 {
            bytes.extend_from_slice(b"end");
        }
        bytes
    }

    #[test]
    fn exact_extremes_with_random_files() {
        let mut seed = 7;
        for _ in 0..500 {
            let (base, current) = (random_file(&mut seed), random_file(&mut seed));
            let hunks = compute(&base, &current);
            let all: Vec<_> = hunks.iter().map(whole).collect();
            let every_line: Vec<_> = hunks
                .iter()
                .map(|h| lines(h, &(0..h.old_lines + h.new_lines).collect::<Vec<_>>()))
                .collect();

            for selection in [&all, &every_line] {
                assert_eq!(partial_content(&base, &current, selection), current);
                assert_eq!(revert_content(&base, &current, selection), base);
            }
            assert_eq!(partial_content(&base, &current, &[]), base);
            assert_eq!(revert_content(&base, &current, &[]), current);
        }
    }

    #[test]
    fn selections_serialize_in_camel_case() {
        let selection = LineSelection {
            hunk: "h".to_string(),
            lines: Some(vec![1, 2]),
        };
        let hunk = Hunk {
            id: "h".to_string(),
            old_start: 1,
            old_lines: 2,
            new_start: 3,
            new_lines: 4,
        };

        assert_eq!(
            serde_json::to_value(&selection).unwrap(),
            serde_json::json!({"hunk": "h", "lines": [1, 2]})
        );
        assert_eq!(
            serde_json::to_value(&hunk).unwrap(),
            serde_json::json!({
                "id": "h", "oldStart": 1, "oldLines": 2, "newStart": 3, "newLines": 4
            })
        );
        let back: LineSelection = serde_json::from_str(r#"{"hunk": "h", "lines": null}"#).unwrap();
        assert_eq!(back.lines, None);
    }

    // Tests with repositories and disk.
    use crate::test_support::TempRepo;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    // With `autocrlf=input`, as on the user's machine: what git stores is not what is on
    // disk when the file has CRLF.
    fn new_repo() -> TempRepo {
        let repo = TempRepo::new();
        repo.git(&["config", "core.autocrlf", "input"]);
        repo
    }

    fn rot13(bytes: &[u8]) -> Vec<u8> {
        bytes
            .iter()
            .map(|byte| match byte {
                b'a'..=b'z' => (byte - b'a' + 13) % 26 + b'a',
                b'A'..=b'Z' => (byte - b'A' + 13) % 26 + b'A',
                other => *other,
            })
            .collect()
    }

    // A filter that changes the stored bytes, and is undone when going back to disk.
    fn rot13_repo(smudge: &str) -> TempRepo {
        let repo = new_repo();
        let tr = "tr 'A-Za-z' 'N-ZA-Mn-za-m'";
        repo.git(&["config", "filter.rot.clean", tr]);
        repo.git(&["config", "filter.rot.smudge", smudge]);
        repo.git(&["config", "filter.rot.required", "true"]);
        write(&repo, ".gitattributes", b"*.txt filter=rot\n");
        repo
    }

    fn write(repo: &TempRepo, file: &str, bytes: &[u8]) {
        fs::write(repo.path().join(file), bytes).unwrap();
    }

    fn read_blob(repo: &TempRepo, object: &str) -> Vec<u8> {
        crate::test_support::git_command()
            .current_dir(repo.path())
            .args(["cat-file", "blob", object])
            .output()
            .unwrap()
            .stdout
    }

    fn read(repo: &TempRepo, file: &str) -> Vec<u8> {
        fs::read(repo.path().join(file)).unwrap()
    }

    fn selection_of(hunks: &[Hunk], which: &[usize]) -> Vec<LineSelection> {
        which.iter().map(|index| whole(&hunks[*index])).collect()
    }

    #[test]
    fn hunks_of_a_file_against_head() {
        let repo = new_repo();
        repo.commit("a.txt", NINE, "one");
        write(&repo, "a.txt", NINE_EDITED);

        let hunks = file_hunks(repo.path(), "a.txt").unwrap();

        assert_eq!(hunks, compute(NINE, NINE_EDITED));
    }

    #[test]
    fn hunks_of_a_new_file_and_of_a_repository_without_commits() {
        let repo = new_repo();
        write(&repo, "new.txt", b"a\nb\n");
        let without_commits = file_hunks(repo.path(), "new.txt").unwrap();
        write(&repo, "other.txt", b"x\n");
        repo.git(&["add", "other.txt"]);
        repo.git(&["commit", "-q", "-m", "one"]);
        let with_commits = file_hunks(repo.path(), "new.txt").unwrap();

        assert_eq!(without_commits, compute(b"", b"a\nb\n"));
        assert_eq!(with_commits, without_commits);
        assert_eq!(without_commits[0].old_lines, 0);
        assert_eq!(without_commits[0].new_lines, 2);
    }

    #[test]
    fn large_or_binary_file_is_not_split() {
        let repo = new_repo();
        let big = vec![b'x'; diff::MAX_DIFF_BYTES as usize + 1];
        repo.commit("large.txt", b"one\n", "large");
        write(&repo, "large.txt", &big);
        repo.commit("heavy.txt", &big, "heavy");
        write(&repo, "heavy.txt", b"small\n");
        repo.commit("bin.dat", b"a\0b\n", "binary");
        write(&repo, "bin.dat", b"a\n");
        repo.commit("other.dat", b"a\n", "text");
        write(&repo, "other.dat", b"a\0b\n");

        for file in ["large.txt", "heavy.txt", "bin.dat", "other.dat"] {
            assert_eq!(file_hunks(repo.path(), file).unwrap(), [], "{file}");
        }
    }

    #[test]
    fn deleted_file_or_link_is_not_split() {
        let repo = new_repo();
        repo.commit("a.txt", b"1\n", "one");
        fs::remove_file(repo.path().join("a.txt")).unwrap();
        write(&repo, "real.txt", b"2\n");
        std::os::unix::fs::symlink("real.txt", repo.path().join("link")).unwrap();

        assert_eq!(file_hunks(repo.path(), "a.txt").unwrap(), []);
        assert_eq!(file_hunks(repo.path(), "link").unwrap(), []);
    }

    #[test]
    fn paths_outside_the_repository_are_rejected() {
        let repo = new_repo();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("x.txt"), b"secret\n").unwrap();
        std::os::unix::fs::symlink(outside.path(), repo.path().join("outlink")).unwrap();

        for path in [
            "../x.txt",
            "/etc/passwd",
            "outlink/x.txt",
            ".git/config",
            "",
        ] {
            let result = file_hunks(repo.path(), path);
            assert!(
                matches!(result, Err(HunkError::OutsideRepository(_))),
                "{path}: {result:?}"
            );
        }
    }

    #[test]
    fn partial_content_keeps_non_utf8_bytes() {
        let repo = new_repo();
        let base = b"a\n\xff\xfe\nc\n\x80\rz\n";
        let current = b"a\n\xff\xff\nc\n\x81\rz\n";
        repo.commit("a.bin.txt", base, "one");
        write(&repo, "a.bin.txt", current);
        let hunks = file_hunks(repo.path(), "a.bin.txt").unwrap();

        let file = partial_file(repo.path(), "a.bin.txt", &selection_of(&hunks, &[1])).unwrap();

        assert_eq!(file.path, "a.bin.txt");
        assert_eq!(file.content, b"a\n\xff\xfe\nc\n\x81\rz\n");
    }

    #[test]
    fn partial_content_of_a_lost_hunk_fails() {
        let repo = new_repo();
        repo.commit("a.txt", NINE, "one");
        write(&repo, "a.txt", NINE_EDITED);
        let hunks = file_hunks(repo.path(), "a.txt").unwrap();
        write(&repo, "a.txt", b"1\ntwo\n3\n4\n5\n6\n7\neighty\n9\n");

        let result = partial_file(repo.path(), "a.txt", &selection_of(&hunks, &[1]));

        match result {
            Err(HunkError::Lost { hunks: lost, .. }) => assert_eq!(lost, [hunks[1].id.clone()]),
            other => panic!("expected Lost: {other:?}"),
        }
    }

    #[test]
    fn partial_content_of_a_nonexistent_line_fails() {
        let repo = new_repo();
        repo.commit("a.txt", NINE, "one");
        write(&repo, "a.txt", NINE_EDITED);
        let hunks = file_hunks(repo.path(), "a.txt").unwrap();

        let result = partial_file(repo.path(), "a.txt", &[lines(&hunks[0], &[2])]);

        assert!(
            matches!(result, Err(HunkError::OutOfRange(_))),
            "{result:?}"
        );
    }

    #[test]
    fn partial_content_of_a_binary_fails() {
        let repo = new_repo();
        repo.commit("bin.dat", b"a\0\n", "one");
        write(&repo, "bin.dat", b"b\0\n");

        let result = partial_file(repo.path(), "bin.dat", &[]);

        assert!(
            matches!(result, Err(HunkError::NotSplittable(_))),
            "{result:?}"
        );
    }

    #[test]
    fn line_rollback_undoes_only_what_was_chosen() {
        let repo = new_repo();
        let dir = TempDir::new().unwrap();
        let base = b"a\n\xff\nc\nd";
        let current = b"a\n\xfe\nc\nD";
        repo.commit("a.txt", base, "one");
        write(&repo, "a.txt", current);
        let hunks = file_hunks(repo.path(), "a.txt").unwrap();

        rollback_lines(
            repo.path(),
            dir.path(),
            "a.txt",
            &selection_of(&hunks, &[1]),
        )
        .unwrap();

        assert_eq!(read(&repo, "a.txt"), b"a\n\xfe\nc\nd");
    }

    #[test]
    fn line_rollback_creates_snapshot() {
        let repo = new_repo();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", NINE, "one");
        write(&repo, "a.txt", NINE_EDITED);
        let hunks = file_hunks(repo.path(), "a.txt").unwrap();

        let id = rollback_lines(
            repo.path(),
            dir.path(),
            "a.txt",
            &selection_of(&hunks, &[0]),
        )
        .unwrap();

        assert_ne!(read(&repo, "a.txt"), NINE_EDITED);
        let copy = snapshots::stored(dir.path(), &id, "a.txt", false).unwrap();
        assert_eq!(fs::read(copy).unwrap(), NINE_EDITED);
        let listed = snapshots::list(dir.path());
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].reason, BEFORE_LINES_ROLLBACK);
    }

    #[test]
    fn line_rollback_keeps_permissions() {
        let repo = new_repo();
        let dir = TempDir::new().unwrap();
        repo.commit("run.sh", b"#!/bin/sh\necho 1\n", "one");
        let file = repo.path().join("run.sh");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
        write(&repo, "run.sh", b"#!/bin/sh\necho 2\n");
        let hunks = file_hunks(repo.path(), "run.sh").unwrap();

        rollback_lines(
            repo.path(),
            dir.path(),
            "run.sh",
            &selection_of(&hunks, &[0]),
        )
        .unwrap();

        assert_eq!(read(&repo, "run.sh"), b"#!/bin/sh\necho 1\n");
        let mode = fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }

    #[test]
    fn line_rollback_if_changed_does_not_write() {
        let repo = new_repo();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", NINE, "one");
        write(&repo, "a.txt", NINE_EDITED);
        let hunks = file_hunks(repo.path(), "a.txt").unwrap();
        let devin = b"1\ntwo\n3\n4\n5\n6\n7\neight\n9\nten\n";

        let result = rollback_lines_with(
            repo.path(),
            dir.path(),
            "a.txt",
            &selection_of(&hunks, &[0]),
            || write(&repo, "a.txt", devin),
        );

        match result {
            Err(error @ HunkError::Changed(_)) => {
                assert!(error.to_string().contains("changed"))
            }
            other => panic!("expected Changed: {other:?}"),
        }
        assert_eq!(read(&repo, "a.txt"), devin);
        let names: Vec<_> = fs::read_dir(repo.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| name != ".git")
            .collect();
        assert_eq!(names, ["a.txt"]);
    }

    #[test]
    fn line_rollback_of_a_lost_hunk_touches_nothing() {
        let repo = new_repo();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", NINE, "one");
        write(&repo, "a.txt", NINE_EDITED);
        let hunks = file_hunks(repo.path(), "a.txt").unwrap();
        let devin = b"1\ntwo\n3\n4\n5\n6\n7\neighty\n9\n";
        write(&repo, "a.txt", devin);

        let result = rollback_lines(
            repo.path(),
            dir.path(),
            "a.txt",
            &selection_of(&hunks, &[1]),
        );

        assert!(matches!(result, Err(HunkError::Lost { .. })), "{result:?}");
        assert_eq!(read(&repo, "a.txt"), devin);
        assert!(snapshots::list(dir.path()).is_empty());
    }

    #[test]
    fn line_rollback_without_selection_does_nothing() {
        let repo = new_repo();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", NINE, "one");
        write(&repo, "a.txt", NINE_EDITED);
        let hunks = file_hunks(repo.path(), "a.txt").unwrap();

        for selection in [vec![], vec![lines(&hunks[0], &[])]] {
            let result = rollback_lines(repo.path(), dir.path(), "a.txt", &selection);
            assert!(
                matches!(result, Err(HunkError::NothingSelected)),
                "{result:?}"
            );
        }
        assert_eq!(read(&repo, "a.txt"), NINE_EDITED);
        assert!(snapshots::list(dir.path()).is_empty());
    }

    #[test]
    fn line_rollback_rejects_foreign_paths_and_binaries() {
        let repo = new_repo();
        let dir = TempDir::new().unwrap();
        repo.commit("bin.dat", b"a\0\n", "one");
        write(&repo, "bin.dat", b"b\0\n");

        let outside = rollback_lines(repo.path(), dir.path(), "../x", &[]);
        let binary = rollback_lines(repo.path(), dir.path(), "bin.dat", &[]);

        assert!(matches!(outside, Err(HunkError::OutsideRepository(_))));
        assert!(matches!(binary, Err(HunkError::NotSplittable(_))));
        assert_eq!(read(&repo, "bin.dat"), b"b\0\n");
    }

    #[test]
    fn crlf_with_autocrlf_input_gives_only_the_real_change() {
        let repo = new_repo();
        repo.commit("a.txt", b"1\n2\n3\n4\n5\n", "one");

        write(&repo, "a.txt", b"1\r\n2\r\n3\r\n4\r\n5\r\n");
        let clean = file_hunks(repo.path(), "a.txt").unwrap();
        write(&repo, "a.txt", b"1\r\n2\r\nthree\r\n4\r\n5\r\n");
        let hunks = file_hunks(repo.path(), "a.txt").unwrap();

        assert_eq!(clean, []);
        assert_eq!(hunks.len(), 1);
        assert_eq!((hunks[0].old_lines, hunks[0].new_lines), (1, 1));
        let file = partial_file(repo.path(), "a.txt", &selection_of(&hunks, &[0])).unwrap();
        assert_eq!(file.content, b"1\n2\nthree\n4\n5\n");
    }

    #[test]
    fn eol_crlf_a_clean_file_has_no_hunks() {
        let repo = new_repo();
        write(&repo, ".gitattributes", b"a.txt text eol=crlf\n");
        repo.commit("a.txt", b"1\r\n2\r\n3\r\n", "one");
        assert_eq!(repo.git(&["show", "HEAD:a.txt"]), "1\n2\n3\n");

        let clean = file_hunks(repo.path(), "a.txt").unwrap();
        write(&repo, "a.txt", b"1\r\ntwo\r\n3\r\n");
        let hunks = file_hunks(repo.path(), "a.txt").unwrap();

        assert_eq!(clean, []);
        assert_eq!(hunks.len(), 1);
        assert_eq!((hunks[0].old_lines, hunks[0].new_lines), (1, 1));
    }

    #[test]
    fn rollback_with_eol_crlf_leaves_the_file_in_crlf() {
        let repo = new_repo();
        let dir = TempDir::new().unwrap();
        write(&repo, ".gitattributes", b"a.txt text eol=crlf\n");
        repo.commit("a.txt", b"1\r\n2\r\n3\r\n4\r\n5\r\n", "one");
        write(&repo, "a.txt", b"1\r\ntwo\r\n3\r\n4\r\nfive\r\n");
        let hunks = file_hunks(repo.path(), "a.txt").unwrap();

        rollback_lines(
            repo.path(),
            dir.path(),
            "a.txt",
            &selection_of(&hunks, &[0]),
        )
        .unwrap();

        assert_eq!(read(&repo, "a.txt"), b"1\r\n2\r\n3\r\n4\r\nfive\r\n");
    }

    // With `autocrlf=input` git does not return CRLF to the disk: the file ends up as a
    // checkout would leave it, in the form git stores.
    #[test]
    fn rollback_with_autocrlf_input_writes_gits_form() {
        let repo = new_repo();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"a\nb\nc\n", "one");
        write(&repo, "a.txt", b"a\r\nB\r\nc\r\nd\r\n");
        let hunks = file_hunks(repo.path(), "a.txt").unwrap();
        assert_eq!(hunks.len(), 2);

        rollback_lines(
            repo.path(),
            dir.path(),
            "a.txt",
            &selection_of(&hunks, &[0]),
        )
        .unwrap();

        assert_eq!(read(&repo, "a.txt"), b"a\nb\nc\nd\n");
    }

    #[test]
    fn with_filter_the_hunks_and_the_partial_go_in_the_stored_form() {
        let repo = rot13_repo("tr 'A-Za-z' 'N-ZA-Mn-za-m'");
        let base = b"one\ntwo\nthree\nfour\nfive\nsix\nseven\n";
        repo.commit("a.txt", base, "one");
        assert_eq!(read_blob(&repo, "HEAD:a.txt"), rot13(base));
        assert_eq!(file_hunks(repo.path(), "a.txt").unwrap(), []);
        let current = b"one\nTWO\nthree\nfour\nfive\nSIX\nseven\n";
        write(&repo, "a.txt", current);
        let hunks = file_hunks(repo.path(), "a.txt").unwrap();
        assert_eq!(hunks.len(), 2);

        let file = partial_file(repo.path(), "a.txt", &selection_of(&hunks, &[1])).unwrap();

        let expected = b"one\ntwo\nthree\nfour\nfive\nSIX\nseven\n";
        assert_eq!(file.content, rot13(expected));
        assert_eq!(hunks, compute(&rot13(base), &rot13(current)));
    }

    #[test]
    fn with_filter_the_rollback_writes_the_disk_form() {
        let repo = rot13_repo("tr 'A-Za-z' 'N-ZA-Mn-za-m'");
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"one\ntwo\nthree\nfour\nfive\nsix\nseven\n", "one");
        write(&repo, "a.txt", b"one\nTWO\nthree\nfour\nfive\nSIX\nseven\n");
        let hunks = file_hunks(repo.path(), "a.txt").unwrap();

        rollback_lines(
            repo.path(),
            dir.path(),
            "a.txt",
            &selection_of(&hunks, &[0]),
        )
        .unwrap();

        assert_eq!(
            read(&repo, "a.txt"),
            b"one\ntwo\nthree\nfour\nfive\nSIX\nseven\n"
        );
    }

    #[test]
    fn rollback_if_it_cannot_return_to_the_disk_form_does_not_write() {
        let repo = rot13_repo("false");
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"one\ntwo\n", "one");
        let devin = b"one\nTWO\n";
        write(&repo, "a.txt", devin);
        let hunks = file_hunks(repo.path(), "a.txt").unwrap();

        let result = rollback_lines(
            repo.path(),
            dir.path(),
            "a.txt",
            &selection_of(&hunks, &[0]),
        );

        match result {
            Err(error @ HunkError::Convert { .. }) => {
                assert!(error.to_string().contains("a.txt"), "{error}")
            }
            other => panic!("expected Convert: {other:?}"),
        }
        assert_eq!(read(&repo, "a.txt"), devin);
        assert!(snapshots::list(dir.path()).is_empty());
    }

    #[test]
    fn link_or_folder_in_head_is_not_split() {
        let repo = new_repo();
        std::os::unix::fs::symlink("target", repo.path().join("link")).unwrap();
        write(&repo, "target", b"1\n");
        fs::create_dir(repo.path().join("folder")).unwrap();
        write(&repo, "folder/x.txt", b"1\n");
        repo.commit("other.txt", b"1\n", "one");
        // On disk they are now regular files.
        fs::remove_file(repo.path().join("link")).unwrap();
        write(&repo, "link", b"2\n");
        fs::remove_dir_all(repo.path().join("folder")).unwrap();
        write(&repo, "folder", b"2\n");

        for path in ["link", "folder"] {
            assert_eq!(file_hunks(repo.path(), path).unwrap(), [], "{path}");
            let result = partial_file(repo.path(), path, &[]);
            assert!(
                matches!(result, Err(HunkError::NotSplittable(_))),
                "{path}: {result:?}"
            );
        }
    }

    #[test]
    fn partial_content_with_nothing_chosen_fails() {
        let repo = new_repo();
        repo.commit("a.txt", NINE, "one");
        write(&repo, "a.txt", NINE_EDITED);
        let hunks = file_hunks(repo.path(), "a.txt").unwrap();

        for selection in [vec![], vec![lines(&hunks[0], &[]), lines(&hunks[1], &[])]] {
            let result = partial_file(repo.path(), "a.txt", &selection);
            assert!(
                matches!(result, Err(HunkError::NothingSelected)),
                "{result:?}"
            );
        }
        let some = partial_file(repo.path(), "a.txt", &[lines(&hunks[0], &[1])]);
        assert!(some.is_ok());
    }

    #[test]
    fn the_lost_selection_carries_the_path() {
        let repo = new_repo();
        repo.commit("a.txt", NINE, "one");
        write(&repo, "a.txt", NINE_EDITED);
        let hunks = file_hunks(repo.path(), "a.txt").unwrap();
        write(&repo, "a.txt", b"1\ntwo\n3\n4\n5\n6\n7\neighty\n9\n");

        let result = partial_file(repo.path(), "a.txt", &selection_of(&hunks, &[1]));

        match result {
            Err(HunkError::Lost { path, hunks: lost }) => {
                assert_eq!(path, "a.txt");
                assert_eq!(lost, [hunks[1].id.clone()]);
            }
            other => panic!("expected Lost: {other:?}"),
        }
    }

    #[test]
    fn lines_is_mandatory_when_reading_a_selection() {
        assert!(serde_json::from_str::<LineSelection>(r#"{"hunk": "h"}"#).is_err());
        let all: LineSelection = serde_json::from_str(r#"{"hunk": "h", "lines": null}"#).unwrap();
        let some: LineSelection = serde_json::from_str(r#"{"hunk": "h", "lines": [1]}"#).unwrap();
        assert_eq!(all.lines, None);
        assert_eq!(some.lines, Some(vec![1]));
    }

    #[test]
    fn losing_lines_looks_at_the_order() {
        assert!(loses_lines(b"b\na\n", &[b"a\nb\n", b"a\nb\nc\n"]));
        assert!(!loses_lines(b"a\nb\n", &[b"a\nb\nc\n"]));
        assert!(!loses_lines(b"a\nx\nb\n", &[b"a\nb\n", b"x\n"]));
        assert!(loses_lines(b"a\na\nb\n", &[b"a\nb\n", b"a\nb\n"]));
    }
}
