use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::commit;
use crate::diff::{self, Side};
use crate::git;
use crate::hunks::{self, Hunk, HunkError};
use crate::snapshots::{self, SnapshotId};
use crate::staging;
use crate::storage;

// The two versions of a working directory file. Both are in the form git
// stores, like `hunks`, so the hunks' line numbers are valid for this text.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeSides {
    pub old: Side,
    pub new: Side,
    pub hunks: Vec<Hunk>,
    // Only if saving the displayed text leaves on disk the same thing that was there.
    pub editable: bool,
    // Why the local side is read-only, if it is not evident from the side itself.
    pub note: Option<String>,
    // Of the bytes on disk, for `write`; `None` if it is not a regular file.
    pub hash: Option<String>,
    // With a conflict there is no stage to compare: HEAD is shown against the disk, without actions.
    pub conflicted: bool,
}

const NOTE_NOT_UTF8: &str = "Read-only: not UTF-8, and saving it from here would change its bytes";
const NOTE_CONVERTED: &str =
    "Read-only: shown as git stores it, different from the disk (line endings or filters)";
const NOTE_SYMLINK: &str = "Read-only: it is a symbolic link";
const NOTE_NOT_SPLIT: &str = "Read-only: cannot be compared by lines";
const NOTE_STAGED: &str = "Read-only: this is what is in the stage";
const NOTE_UNMERGED: &str = "Read-only: with a conflict, HEAD is compared with the disk";

// The two comparisons of staging area mode: HEAD with the stage, and the stage with the disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Comparison {
    Staged,
    Local,
}

pub fn content_hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

// The path must stay inside the repository once links are resolved, also the file's own
// link, which `check_path` does not follow because a rollback can deal with that link.
pub fn check_inside(repo: &Path, path: &str) -> Result<(), HunkError> {
    let outside = || HunkError::OutsideRepository(path.to_string());
    snapshots::check_path(repo, path).map_err(|error| match error.kind() {
        io::ErrorKind::InvalidInput => outside(),
        _ => HunkError::Io(error),
    })?;
    let full = repo.join(path);
    if !fs::symlink_metadata(&full).is_ok_and(|meta| meta.is_symlink()) {
        return Ok(());
    }
    let real = fs::canonicalize(&full).map_err(|_| outside())?;
    if real.starts_with(fs::canonicalize(repo)?) {
        Ok(())
    } else {
        Err(outside())
    }
}

fn head_side(repo: &Path, path: &str) -> Result<Side, HunkError> {
    if !git::head_exists(repo)? {
        return Ok(Side::Absent);
    }
    Ok(diff::read_side(repo, "HEAD", path)?)
}

fn lossy(bytes: &[u8]) -> Side {
    Side::Text {
        content: String::from_utf8_lossy(bytes).into_owned(),
    }
}

fn is_editable(parts: &hunks::Sides) -> bool {
    parts.stored == parts.disk.bytes && std::str::from_utf8(&parts.stored).is_ok()
}

// What is on disk when it cannot be split into hunks: the side is treated as a whole.
fn unsplit_side(repo: &Path, path: &str) -> Result<(Side, Option<&'static str>), HunkError> {
    let full = repo.join(path);
    let meta = match fs::symlink_metadata(&full) {
        Ok(meta) => meta,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok((Side::Absent, None))
        }
        Err(error) => return Err(error.into()),
    };
    if meta.is_symlink() {
        let target = fs::read_link(&full)?;
        return Ok((
            lossy(target.as_os_str().as_encoded_bytes()),
            Some(NOTE_SYMLINK),
        ));
    }
    if meta.is_dir() {
        return Ok(match hunks::stored_gitlink(repo, path)? {
            Some(hash) => (diff::gitlink_side(&hash), None),
            None => (Side::Binary, None),
        });
    }
    if meta.len() > diff::MAX_DIFF_BYTES {
        return Ok((Side::TooLarge { bytes: meta.len() }, None));
    }
    let bytes = fs::read(&full)?;
    if diff::looks_binary(&bytes) {
        return Ok((Side::Binary, None));
    }
    Ok((lossy(&bytes), Some(NOTE_NOT_SPLIT)))
}

fn split_sides(old: Side, parts: &hunks::Sides, hunks: Vec<Hunk>) -> WorktreeSides {
    let note = if std::str::from_utf8(&parts.stored).is_err() {
        Some(NOTE_NOT_UTF8)
    } else if parts.stored != parts.disk.bytes {
        Some(NOTE_CONVERTED)
    } else {
        None
    };
    WorktreeSides {
        old,
        new: lossy(&parts.stored),
        hunks,
        editable: is_editable(parts),
        note: note.map(str::to_string),
        hash: Some(content_hash(&parts.disk.bytes)),
        conflicted: false,
    }
}

fn unsplit_sides(repo: &Path, path: &str, old: Side) -> Result<WorktreeSides, HunkError> {
    let (new, note) = unsplit_side(repo, path)?;
    let regular = fs::symlink_metadata(repo.join(path)).is_ok_and(|meta| meta.is_file());
    let hash = if regular && matches!(new, Side::Text { .. } | Side::Binary) {
        Some(content_hash(&fs::read(repo.join(path))?))
    } else {
        None
    };
    Ok(WorktreeSides {
        old,
        new,
        hunks: Vec::new(),
        editable: false,
        note: note.map(str::to_string),
        hash,
        conflicted: false,
    })
}

pub fn sides(repo: &Path, path: &str) -> Result<WorktreeSides, HunkError> {
    check_inside(repo, path)?;
    let old = head_side(repo, path)?;
    match hunks::read_sides(repo, path)? {
        Some(parts) => {
            let found = hunks::compute(&parts.base, &parts.stored);
            Ok(split_sides(old, &parts, found))
        }
        None => unsplit_sides(repo, path, old),
    }
}

fn index_side(repo: &Path, path: &str) -> Result<Side, HunkError> {
    Ok(match hunks::index_gitlink(repo, path)? {
        Some(hash) => diff::gitlink_side(&hash),
        None => diff::read_side(repo, "", path)?,
    })
}

// The left and right sides of each comparison. Only the disk can be edited. A file in
// conflict has no entry to compare: until the phase 3 merge dialog arrives,
// HEAD is shown against the disk, to look at, without actions.
pub fn stage_sides(
    repo: &Path,
    path: &str,
    comparison: Comparison,
) -> Result<WorktreeSides, HunkError> {
    check_inside(repo, path)?;
    if !commit::conflicted(repo, &[path.to_string()])?.is_empty() {
        return Ok(WorktreeSides {
            editable: false,
            note: Some(NOTE_UNMERGED.to_string()),
            conflicted: true,
            ..sides(repo, path)?
        });
    }
    if comparison == Comparison::Local {
        let old = index_side(repo, path)?;
        return match staging::unstaged_view(repo, path)? {
            Some((staged, parts)) => {
                let found = hunks::compute(&staged, &parts.stored);
                Ok(split_sides(old, &parts, found))
            }
            None => unsplit_sides(repo, path, old),
        };
    }
    let hunks = staging::staged_view(repo, path)?
        .map(|(head, staged)| hunks::compute(&head, &staged))
        .unwrap_or_default();
    Ok(WorktreeSides {
        old: head_side(repo, path)?,
        new: index_side(repo, path)?,
        hunks,
        editable: false,
        note: Some(NOTE_STAGED.to_string()),
        hash: None,
        conflicted: false,
    })
}

// The hunks the file would have with `content`, without saving it: the ones seen while editing.
pub fn preview(repo: &Path, path: &str, content: &str) -> Result<Vec<Hunk>, HunkError> {
    check_inside(repo, path)?;
    Ok(hunks::compute(
        &hunks::head_bytes(repo, path)?,
        content.as_bytes(),
    ))
}

// Like `preview`, in the comparison of the stage with the disk.
pub fn stage_preview(repo: &Path, path: &str, content: &str) -> Result<Vec<Hunk>, HunkError> {
    check_inside(repo, path)?;
    let staged = hunks::read_index(repo, path)?.unwrap_or_default();
    Ok(hunks::compute(&staged, content.as_bytes()))
}

// Saves what was edited on the local side, if the file is still the one that was read. It keeps no copy:
// the user is editing the file. The caller must hold the operations lock.
pub fn write(
    repo: &Path,
    path: &str,
    content: &str,
    expected_hash: &str,
) -> Result<String, HunkError> {
    check_inside(repo, path)?;
    let parts =
        hunks::read_sides(repo, path)?.ok_or_else(|| HunkError::NotEditable(path.to_string()))?;
    if !is_editable(&parts) && content_hash(&parts.disk.bytes) == expected_hash {
        return Err(HunkError::NotEditable(path.to_string()));
    }
    // It is checked again: the stored form takes a while, and Devin may have written in the meantime.
    let now = hunks::read_current(repo, path)?;
    let Some(now) = now.filter(|now| content_hash(&now.bytes) == expected_hash) else {
        return Err(HunkError::Changed(path.to_string()));
    };
    storage::write_atomic_with(&repo.join(path), content.as_bytes(), Some(now.permissions))?;
    Ok(content_hash(content.as_bytes()))
}

// HEAD, the stage and the disk of a file, to compare them at once. The stage and the disk are edited,
// and their hashes are what is expected when writing them: the blob of the index entry and the SHA-256 of the
// bytes on disk.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreeSides {
    pub head: String,
    pub staged: String,
    pub local: String,
    pub staged_hash: String,
    pub local_hash: String,
}

// What was written: the hash expected on the next write, and the snapshot saved before, if any.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Written {
    pub hash: String,
    pub copy: Option<SnapshotId>,
}

const REGULAR_MODES: [&str; 2] = ["100644", "100755"];

struct IndexEntry {
    mode: String,
    oid: String,
}

// The normal (stage 0) index entry for `path`. With a conflict there is none to edit.
fn index_entry(repo: &Path, path: &str) -> Result<Option<IndexEntry>, HunkError> {
    let args = git::exact(&["--literal-pathspecs", "ls-files", "-s", "-z", "--", path]);
    let listed = git::run(repo, &args)?;
    let record = listed.split(|byte| *byte == 0).next().unwrap_or_default();
    let head = record
        .split(|byte| *byte == b'\t')
        .next()
        .unwrap_or_default();
    let fields: Vec<String> = String::from_utf8_lossy(head)
        .split_whitespace()
        .map(str::to_string)
        .collect();
    match fields.as_slice() {
        [] => Ok(None),
        [_, _, stage] if stage != "0" => Err(HunkError::Unmerged(path.to_string())),
        [mode, oid, _] => Ok(Some(IndexEntry {
            mode: mode.clone(),
            oid: oid.clone(),
        })),
        _ => Err(io::Error::other(format!("Invalid output from git ls-files for {path}")).into()),
    }
}

// `GitFileStatus.path(HEAD)`: in a staged rename, HEAD has the file at the previous path.
fn head_path(repo: &Path, path: &str) -> Result<String, HunkError> {
    if !git::head_exists(repo)? {
        return Ok(path.to_string());
    }
    let args = git::exact(&[
        "diff-index",
        "--cached",
        "-M",
        "--name-status",
        "-z",
        "HEAD",
    ]);
    let output = git::run(repo, &args)?;
    let mut fields = output
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .map(|field| String::from_utf8_lossy(field).into_owned());
    while let Some(status) = fields.next() {
        let first = fields.next().unwrap_or_default();
        if !status.starts_with(['R', 'C']) {
            continue;
        }
        let second = fields.next().unwrap_or_default();
        if second == path && status.starts_with('R') {
            return Ok(first);
        }
    }
    Ok(path.to_string())
}

fn in_head(repo: &Path, path: &str) -> Result<bool, HunkError> {
    if !git::head_exists(repo)? {
        return Ok(false);
    }
    let args = git::exact(&["--literal-pathspecs", "ls-tree", "-z", "HEAD", "--", path]);
    Ok(!git::run(repo, &args)?.is_empty())
}

fn text(bytes: Option<Vec<u8>>, path: &str) -> Result<String, HunkError> {
    bytes
        .filter(|bytes| !diff::looks_binary(bytes))
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(|| HunkError::NoThreeSides(path.to_string()))
}

// `compareThreeVersions`, only if all three versions exist and all three are text. The disk is
// edited as it is: if git stores it in another form (line endings, filters) it cannot be done.
pub fn three_sides(repo: &Path, path: &str) -> Result<ThreeSides, HunkError> {
    check_inside(repo, path)?;
    let missing = || HunkError::NoThreeSides(path.to_string());
    if !commit::conflicted(repo, &[path.to_string()])?.is_empty() {
        return Err(HunkError::Unmerged(path.to_string()));
    }
    let origin = head_path(repo, path)?;
    let entry = index_entry(repo, path)?.ok_or_else(missing)?;
    if !in_head(repo, &origin)? || !REGULAR_MODES.contains(&entry.mode.as_str()) {
        return Err(missing());
    }
    let head = text(hunks::read_head(repo, &origin)?, path)?;
    let staged = text(hunks::read_index(repo, path)?, path)?;
    let disk = hunks::read_current(repo, path)?.ok_or_else(missing)?;
    if hunks::stored_form(repo, path)?.as_deref() != Some(&disk.bytes[..]) {
        return Err(missing());
    }
    let local_hash = content_hash(&disk.bytes);
    let local = text(Some(disk.bytes), path)?;
    Ok(ThreeSides {
        head,
        staged,
        local,
        staged_hash: entry.oid,
        local_hash,
    })
}

// The entry about to be written, if it is still the one that was read.
fn expected_entry(repo: &Path, path: &str, expected_hash: &str) -> Result<IndexEntry, HunkError> {
    let entry = index_entry(repo, path)?
        .filter(|entry| REGULAR_MODES.contains(&entry.mode.as_str()))
        .ok_or_else(|| HunkError::NotEditable(path.to_string()))?;
    if entry.oid != expected_hash {
        return Err(HunkError::Changed(path.to_string()));
    }
    Ok(entry)
}

// `GitIndexUtil.write`: the content as a blob and the index entry pointing to it, with the mode
// it had. It does not write if the entry is no longer the one that was read. The caller must hold the operations
// lock.
pub fn write_index(
    repo: &Path,
    path: &str,
    content: &str,
    expected_hash: &str,
) -> Result<String, HunkError> {
    check_inside(repo, path)?;
    expected_entry(repo, path, expected_hash)?;
    let hash_args = git::exact(&["hash-object", "-w", "--stdin", "--path", path]);
    let blob = git::run_with_input(repo, &hash_args, content.as_bytes())?;
    let blob = String::from_utf8_lossy(&blob).trim().to_string();
    diff::check_hash(&blob)?;
    // It is checked again: Devin may have changed the index in the meantime.
    let entry = expected_entry(repo, path, expected_hash)?;
    let info = format!("{},{blob},{path}", entry.mode);
    git::run_mutating(repo, &git::exact(&["update-index", "--cacheinfo", &info]))?;
    Ok(blob)
}

// Like `write_index`, with a snapshot of the lines that were only in the index, like `unstage_hunks`.
pub fn write_index_copying(
    repo: &Path,
    dir: &Path,
    path: &str,
    content: &str,
    expected_hash: &str,
) -> Result<Written, HunkError> {
    check_inside(repo, path)?;
    expected_entry(repo, path, expected_hash)?;
    let staged = hunks::read_index(repo, path)?.unwrap_or_default();
    let head = hunks::read_head(repo, &head_path(repo, path)?)?.unwrap_or_default();
    let local = hunks::read_current(repo, path)?.map(|disk| disk.bytes);
    let kept = [
        content.as_bytes(),
        &head,
        local.as_deref().unwrap_or_default(),
    ];
    let copy = if hunks::loses_lines(&staged, &kept) {
        let paths = [path.to_string()];
        let reason = staging::BEFORE_STAGE;
        Some(snapshots::create_for_index(dir, repo, &paths, reason).map_err(HunkError::Snapshot)?)
    } else {
        None
    };
    let hash = write_index(repo, path, content, expected_hash)?;
    Ok(Written { hash, copy })
}

// `Revert` of a range: it changes the file on disk, so it always saves a snapshot first, like
// `staging::revert`.
pub fn write_reverting(
    repo: &Path,
    dir: &Path,
    path: &str,
    content: &str,
    expected_hash: &str,
) -> Result<Written, HunkError> {
    check_inside(repo, path)?;
    let now = hunks::read_current(repo, path)?;
    if !now.is_some_and(|now| content_hash(&now.bytes) == expected_hash) {
        return Err(HunkError::Changed(path.to_string()));
    }
    let paths = [path.to_string()];
    let copy = snapshots::create(dir, repo, &paths, staging::BEFORE_REVERT)
        .map_err(HunkError::Snapshot)?;
    let hash = write(repo, path, content, expected_hash)?;
    Ok(Written {
        hash,
        copy: Some(copy),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::MAX_DIFF_BYTES;
    use crate::test_support::TempRepo;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use tempfile::TempDir;

    fn text(side: &Side) -> &str {
        match side {
            Side::Text { content } => content,
            other => panic!("expected text: {other:?}"),
        }
    }

    fn write_file(repo: &TempRepo, path: &str, bytes: &[u8]) {
        fs::write(repo.path().join(path), bytes).unwrap();
    }

    #[test]
    fn sides_of_modified_file() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\ntwo\n", "first");
        write_file(&repo, "a.txt", b"one\nthree\n");

        let found = sides(repo.path(), "a.txt").unwrap();

        assert_eq!(text(&found.old), "one\ntwo\n");
        assert_eq!(text(&found.new), "one\nthree\n");
        assert!(found.editable);
        assert_eq!(found.note, None);
        assert_eq!(
            found.hunks,
            hunks::file_hunks(repo.path(), "a.txt").unwrap()
        );
        assert_eq!(found.hash, Some(content_hash(b"one\nthree\n")));
    }

    #[test]
    fn sides_of_unversioned_file() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\n", "first");
        write_file(&repo, "new.txt", b"hello\n");

        let found = sides(repo.path(), "new.txt").unwrap();

        assert_eq!(found.old, Side::Absent);
        assert_eq!(text(&found.new), "hello\n");
        assert!(found.editable);
        assert_eq!(found.hunks.len(), 1);
    }

    #[test]
    fn sides_in_a_repository_without_commits() {
        let repo = TempRepo::new();
        write_file(&repo, "a.txt", b"hello\n");

        let found = sides(repo.path(), "a.txt").unwrap();

        assert_eq!(found.old, Side::Absent);
        assert_eq!(text(&found.new), "hello\n");
    }

    #[test]
    fn sides_of_deleted_file() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\n", "first");
        fs::remove_file(repo.path().join("a.txt")).unwrap();

        let found = sides(repo.path(), "a.txt").unwrap();

        assert_eq!(text(&found.old), "one\n");
        assert_eq!(found.new, Side::Absent);
        assert!(!found.editable);
        assert_eq!(found.hash, None);
        assert!(found.hunks.is_empty());
    }

    #[test]
    fn sides_of_binary_file_and_of_large_file() {
        let repo = TempRepo::new();
        repo.commit("a.bin", b"ab\0cd", "binary");
        write_file(&repo, "a.bin", b"ab\0ce");
        let large = vec![b'x'; MAX_DIFF_BYTES as usize + 1];
        write_file(&repo, "large.txt", &large);

        let binary = sides(repo.path(), "a.bin").unwrap();
        let big = sides(repo.path(), "large.txt").unwrap();

        assert_eq!(binary.old, Side::Binary);
        assert_eq!(binary.new, Side::Binary);
        assert!(!binary.editable);
        assert_eq!(
            big.new,
            Side::TooLarge {
                bytes: large.len() as u64
            }
        );
        assert!(!big.editable);
    }

    // The local side shows what git stores: it is the form in which the hunks are computed.
    #[test]
    fn with_line_ending_conversion_the_stored_form_is_shown_and_not_editable() {
        let repo = TempRepo::new();
        repo.git(&["config", "core.autocrlf", "input"]);
        repo.commit("a.txt", b"one\ntwo\n", "first");
        write_file(&repo, "a.txt", b"one\r\nthree\r\n");

        let found = sides(repo.path(), "a.txt").unwrap();

        assert_eq!(text(&found.new), "one\nthree\n");
        assert!(!found.editable);
        assert!(found.note.is_some());
        assert_eq!(found.hash, Some(content_hash(b"one\r\nthree\r\n")));
    }

    #[test]
    fn a_non_utf8_file_is_not_editable() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"caf\xe9\n", "first");
        write_file(&repo, "a.txt", b"caf\xe9 only\n");

        let found = sides(repo.path(), "a.txt").unwrap();

        assert!(!found.editable);
        assert!(found.note.is_some());
        assert_eq!(found.hunks.len(), 1);
    }

    #[test]
    fn a_crlf_file_without_conversion_is_editable() {
        let repo = TempRepo::new();
        repo.git(&["config", "core.autocrlf", "false"]);
        repo.commit("a.txt", b"one\r\n", "first");
        write_file(&repo, "a.txt", b"two\r\n");

        let found = sides(repo.path(), "a.txt").unwrap();

        assert_eq!(text(&found.new), "two\r\n");
        assert!(found.editable);
    }

    #[test]
    fn a_crlf_file_with_autocrlf_input_is_not_editable() {
        let repo = TempRepo::new();
        repo.git(&["config", "core.autocrlf", "input"]);
        repo.commit("a.txt", b"one\ntwo\n", "first");
        write_file(&repo, "a.txt", b"one\r\nTWO\r\n");

        let found = sides(repo.path(), "a.txt").unwrap();

        assert!(!found.editable);
    }

    #[test]
    fn a_link_inside_the_repository_shows_its_target() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\n", "first");
        symlink("a.txt", repo.path().join("link")).unwrap();

        let found = sides(repo.path(), "link").unwrap();

        assert_eq!(text(&found.new), "a.txt");
        assert!(!found.editable);
        assert!(found.hunks.is_empty());
    }

    #[test]
    fn gitlink_is_not_absent() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\n", "first");
        let target = "1111111111111111111111111111111111111111";
        repo.git(&[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{target},sub"),
        ]);
        repo.git(&["commit", "-q", "-m", "submodule"]);
        fs::create_dir(repo.path().join("sub")).unwrap();

        let found = sides(repo.path(), "sub").unwrap();

        let shown = format!("Subproject commit {target}\n");
        assert_eq!(text(&found.old), shown);
        assert_eq!(text(&found.new), shown);
        assert!(!found.editable);
    }

    #[test]
    fn damaged_object_gives_error() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\n", "first");
        let blob = repo.git(&["rev-parse", "HEAD:a.txt"]);
        let blob = blob.trim();
        let loose = repo
            .path()
            .join(".git/objects")
            .join(&blob[..2])
            .join(&blob[2..]);
        fs::remove_file(loose).unwrap();

        assert!(sides(repo.path(), "a.txt").is_err());
    }

    #[test]
    fn write_with_correct_hash() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\n", "first");
        let full = repo.path().join("a.txt");
        fs::set_permissions(&full, fs::Permissions::from_mode(0o755)).unwrap();
        let hash = sides(repo.path(), "a.txt").unwrap().hash.unwrap();

        let written = write(repo.path(), "a.txt", "two\n", &hash).unwrap();

        assert_eq!(fs::read(&full).unwrap(), b"two\n");
        assert_eq!(written, content_hash(b"two\n"));
        assert_eq!(
            fs::metadata(&full).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn write_with_old_hash_fails() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\n", "first");
        let hash = sides(repo.path(), "a.txt").unwrap().hash.unwrap();
        write_file(&repo, "a.txt", b"Devin\n");

        let result = write(repo.path(), "a.txt", "mine\n", &hash);

        assert!(matches!(result, Err(HunkError::Changed(_))));
        assert_eq!(fs::read(repo.path().join("a.txt")).unwrap(), b"Devin\n");
    }

    #[test]
    fn writing_a_non_editable_file_fails_and_does_not_touch_its_bytes() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"caf\xe9\n", "first");
        let hash = sides(repo.path(), "a.txt").unwrap().hash.unwrap();

        let result = write(repo.path(), "a.txt", "café\n", &hash);

        assert!(matches!(result, Err(HunkError::NotEditable(_))));
        assert_eq!(fs::read(repo.path().join("a.txt")).unwrap(), b"caf\xe9\n");
    }

    #[test]
    fn paths_outside_the_repository_are_rejected() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\n", "first");
        let outside = TempDir::new().unwrap();
        let secret = outside.path().join("secret.txt");
        fs::write(&secret, b"no\n").unwrap();
        symlink(&secret, repo.path().join("outside")).unwrap();
        symlink(outside.path(), repo.path().join("folder")).unwrap();
        let absolute = secret.to_string_lossy().into_owned();

        for path in [
            "../x",
            absolute.as_str(),
            "outside",
            "folder/secret.txt",
            ".git/config",
        ] {
            assert!(
                matches!(
                    sides(repo.path(), path),
                    Err(HunkError::OutsideRepository(_))
                ),
                "{path}"
            );
            assert!(
                matches!(
                    write(repo.path(), path, "x", "hash"),
                    Err(HunkError::OutsideRepository(_))
                ),
                "{path}"
            );
        }
        assert_eq!(fs::read(&secret).unwrap(), b"no\n");
    }

    #[test]
    fn hunk_preview_as_if_it_were_saved() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\ntwo\n", "first");

        let preview = preview(repo.path(), "a.txt", "one\nthree\n").unwrap();
        write_file(&repo, "a.txt", b"one\nthree\n");

        assert_eq!(preview, hunks::file_hunks(repo.path(), "a.txt").unwrap());
    }

    // The two comparisons of staging area mode.

    const NINE: &[u8] = b"1\n2\n3\n4\n5\n6\n7\n8\n9\n";

    fn mixed(repo: &TempRepo) {
        repo.commit("a.txt", NINE, "base");
        write_file(repo, "a.txt", b"1\nin stage\n3\n4\n5\n6\n7\n8\n9\n");
        repo.git(&["add", "a.txt"]);
        write_file(repo, "a.txt", b"1\nin stage\n3\n4\n5\n6\n7\non disk\n9\n");
    }

    #[test]
    fn compare_head_with_stage() {
        let repo = TempRepo::new();
        mixed(&repo);

        let found = stage_sides(repo.path(), "a.txt", Comparison::Staged).unwrap();

        assert_eq!(text(&found.old).as_bytes(), NINE);
        assert_eq!(text(&found.new), "1\nin stage\n3\n4\n5\n6\n7\n8\n9\n");
        assert_eq!(found.hunks.len(), 1);
        assert_eq!(found.hunks[0].old_start, 1);
        assert!(!found.editable);
        assert_eq!(found.note.as_deref(), Some(NOTE_STAGED));
        assert_eq!(found.hash, None);
    }

    #[test]
    fn compare_stage_with_the_disk() {
        let repo = TempRepo::new();
        mixed(&repo);

        let found = stage_sides(repo.path(), "a.txt", Comparison::Local).unwrap();

        assert_eq!(text(&found.old), "1\nin stage\n3\n4\n5\n6\n7\n8\n9\n");
        assert_eq!(text(&found.new), "1\nin stage\n3\n4\n5\n6\n7\non disk\n9\n");
        assert_eq!(found.hunks.len(), 1);
        assert_eq!(found.hunks[0].old_start, 7);
        assert!(found.editable);
        assert!(found.hash.is_some());
    }

    #[test]
    fn an_unversioned_file_is_not_in_the_stage() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\n", "first");
        write_file(&repo, "new.txt", b"hello\n");

        let staged = stage_sides(repo.path(), "new.txt", Comparison::Staged).unwrap();
        let local = stage_sides(repo.path(), "new.txt", Comparison::Local).unwrap();

        assert_eq!(staged.old, Side::Absent);
        assert_eq!(staged.new, Side::Absent);
        assert_eq!(local.old, Side::Absent);
        assert_eq!(text(&local.new), "hello\n");
        assert_eq!(local.hunks.len(), 1);
    }

    #[test]
    fn a_file_deleted_from_disk_is_compared_whole() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\n", "first");
        fs::remove_file(repo.path().join("a.txt")).unwrap();

        let found = stage_sides(repo.path(), "a.txt", Comparison::Local).unwrap();

        assert_eq!(text(&found.old), "one\n");
        assert_eq!(found.new, Side::Absent);
        assert!(found.hunks.is_empty());
        assert!(!found.editable);
    }

    #[test]
    fn a_binary_is_compared_whole() {
        let repo = TempRepo::new();
        repo.commit("a.bin", b"ab\0cd", "binary");
        write_file(&repo, "a.bin", b"ab\0ce");
        repo.git(&["add", "a.bin"]);

        let found = stage_sides(repo.path(), "a.bin", Comparison::Staged).unwrap();

        assert_eq!(found.old, Side::Binary);
        assert_eq!(found.new, Side::Binary);
        assert!(found.hunks.is_empty());
    }

    #[test]
    fn a_submodule_in_the_stage_is_seen_as_its_commit() {
        let repo = TempRepo::new();
        let hash = repo.commit("a.txt", b"one\n", "first");
        let info = format!("160000,{hash},sub");
        repo.git(&["update-index", "--add", "--cacheinfo", &info]);

        let found = stage_sides(repo.path(), "sub", Comparison::Staged).unwrap();

        assert_eq!(text(&found.new), format!("Subproject commit {hash}\n"));
    }

    #[test]
    fn comparisons_reject_foreign_paths() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\n", "first");

        let result = stage_sides(repo.path(), "../a.txt", Comparison::Local);

        assert!(matches!(result, Err(HunkError::OutsideRepository(_))));
    }

    #[test]
    fn what_is_written_is_compared_with_the_stage() {
        let repo = TempRepo::new();
        mixed(&repo);

        let found = stage_preview(
            repo.path(),
            "a.txt",
            "1\nin stage\n3\n4\n5\n6\n7\n8\nnine\n",
        )
        .unwrap();

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].old_start, 8);
    }

    #[test]
    fn a_conflict_is_compared_from_head_with_the_disk() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"base\n", "base");
        repo.git(&["checkout", "-q", "-b", "other"]);
        repo.commit("a.txt", b"other\n", "on other");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("a.txt", b"main\n", "on main");
        let merge = std::process::Command::new("git")
            .current_dir(repo.path())
            .args(["merge", "-q", "other"])
            .output()
            .unwrap();
        assert!(!merge.status.success());

        for comparison in [Comparison::Local, Comparison::Staged] {
            let found = stage_sides(repo.path(), "a.txt", comparison).unwrap();

            assert_eq!(text(&found.old), "main\n");
            assert!(text(&found.new).contains("<<<<<<<"));
            assert!(found.conflicted);
            assert!(!found.editable);
            assert_eq!(found.note.as_deref(), Some(NOTE_UNMERGED));
        }
    }

    // HEAD, the stage and the disk at once.

    fn index_hash(repo: &TempRepo, path: &str) -> String {
        repo.git(&["rev-parse", &format!(":{path}")])
            .trim()
            .to_string()
    }

    #[test]
    fn three_panes_head_staged_local() {
        let repo = TempRepo::new();
        mixed(&repo);

        let found = three_sides(repo.path(), "a.txt").unwrap();

        assert_eq!(found.head.as_bytes(), NINE);
        assert_eq!(found.staged, "1\nin stage\n3\n4\n5\n6\n7\n8\n9\n");
        assert_eq!(found.local, "1\nin stage\n3\n4\n5\n6\n7\non disk\n9\n");
        assert_eq!(found.staged_hash, index_hash(&repo, "a.txt"));
        assert_eq!(
            found.local_hash,
            content_hash(b"1\nin stage\n3\n4\n5\n6\n7\non disk\n9\n")
        );
    }

    #[test]
    fn a_rename_reads_head_from_the_previous_path() {
        let repo = TempRepo::new();
        repo.commit("old.txt", NINE, "base");
        repo.git(&["mv", "old.txt", "new.txt"]);
        write_file(&repo, "new.txt", b"1\n2\n3\n4\n5\n6\n7\n8\nnine\n");

        let found = three_sides(repo.path(), "new.txt").unwrap();

        assert_eq!(found.head.as_bytes(), NINE);
        assert_eq!(found.staged.as_bytes(), NINE);
        assert_eq!(found.local, "1\n2\n3\n4\n5\n6\n7\n8\nnine\n");
    }

    #[test]
    fn only_with_the_three_versions() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\n", "base");
        repo.commit("b.txt", b"one\n", "base");
        repo.commit("c.txt", b"one\n", "base");
        repo.commit("d.bin", b"ab\0cd", "binary");
        write_file(&repo, "d.bin", b"ab\0ce");
        write_file(&repo, "new.txt", b"hello\n");
        write_file(&repo, "added.txt", b"hello\n");
        repo.git(&["add", "added.txt"]);
        fs::remove_file(repo.path().join("b.txt")).unwrap();
        repo.git(&["rm", "-q", "--cached", "c.txt"]);

        for path in ["new.txt", "added.txt", "b.txt", "c.txt", "d.bin"] {
            assert!(
                matches!(
                    three_sides(repo.path(), path),
                    Err(HunkError::NoThreeSides(_))
                ),
                "{path}: {:?}",
                three_sides(repo.path(), path).map(drop)
            );
        }
        assert!(three_sides(repo.path(), "a.txt").is_ok());
    }

    #[test]
    fn with_line_ending_conversion_there_are_no_three_panes() {
        let repo = TempRepo::new();
        repo.git(&["config", "core.autocrlf", "input"]);
        repo.commit("a.txt", b"one\ntwo\n", "first");
        write_file(&repo, "a.txt", b"one\r\nthree\r\n");

        let result = three_sides(repo.path(), "a.txt");

        assert!(matches!(result, Err(HunkError::NoThreeSides(_))));
    }

    #[test]
    fn stage_of_a_range_writes_the_index() {
        let repo = TempRepo::new();
        mixed(&repo);
        let full = repo.path().join("a.txt");
        fs::set_permissions(&full, fs::Permissions::from_mode(0o755)).unwrap();
        repo.git(&["update-index", "--chmod=+x", "a.txt"]);
        let found = three_sides(repo.path(), "a.txt").unwrap();

        let staged = "1\nin stage\n3\n4\n5\n6\n7\non disk\n9\n";
        let hash = write_index(repo.path(), "a.txt", staged, &found.staged_hash).unwrap();

        assert_eq!(repo.git(&["show", ":a.txt"]), staged);
        assert_eq!(hash, index_hash(&repo, "a.txt"));
        assert!(repo
            .git(&["ls-files", "-s", "a.txt"])
            .starts_with("100755 "));
        assert_eq!(fs::read(&full).unwrap(), staged.as_bytes());
        assert_eq!(repo.git(&["status", "--porcelain"]), "M  a.txt\n");
    }

    #[test]
    fn unstage_of_a_range_returns_head_to_the_index() {
        let repo = TempRepo::new();
        mixed(&repo);
        let found = three_sides(repo.path(), "a.txt").unwrap();

        let dir = TempDir::new().unwrap();
        let written = write_index_copying(
            repo.path(),
            dir.path(),
            "a.txt",
            &found.head,
            &found.staged_hash,
        )
        .unwrap();

        assert_eq!(repo.git(&["diff", "--cached", "--name-only"]), "");
        assert_eq!(written.hash, index_hash(&repo, "a.txt"));
        // What leaves the index is still on disk: no snapshot is needed.
        assert_eq!(written.copy, None);
    }

    #[test]
    fn taking_out_of_the_index_what_is_not_elsewhere_saves_a_snapshot() {
        let repo = TempRepo::new();
        mixed(&repo);
        write_file(&repo, "a.txt", NINE);
        let found = three_sides(repo.path(), "a.txt").unwrap();
        let dir = TempDir::new().unwrap();

        let written = write_index_copying(
            repo.path(),
            dir.path(),
            "a.txt",
            &found.head,
            &found.staged_hash,
        )
        .unwrap();

        let copy = written.copy.expect("snapshot");
        let stored = snapshots::stored(dir.path(), &copy, "a.txt", true).unwrap();
        assert_eq!(
            fs::read(stored).unwrap(),
            b"1\nin stage\n3\n4\n5\n6\n7\n8\n9\n"
        );
    }

    #[test]
    fn revert_of_a_range_changes_the_file_with_a_snapshot() {
        let repo = TempRepo::new();
        mixed(&repo);
        let found = three_sides(repo.path(), "a.txt").unwrap();
        let dir = TempDir::new().unwrap();

        let written = write_reverting(
            repo.path(),
            dir.path(),
            "a.txt",
            &found.staged,
            &found.local_hash,
        )
        .unwrap();

        let disk = fs::read(repo.path().join("a.txt")).unwrap();
        assert_eq!(disk, found.staged.as_bytes());
        assert_eq!(written.hash, content_hash(&disk));
        let copy = written.copy.expect("snapshot");
        let stored = snapshots::stored(dir.path(), &copy, "a.txt", false).unwrap();
        assert_eq!(fs::read(stored).unwrap(), found.local.as_bytes());
    }

    #[test]
    fn revert_with_the_file_changed_outside_writes_neither_file_nor_snapshot() {
        let repo = TempRepo::new();
        mixed(&repo);
        let found = three_sides(repo.path(), "a.txt").unwrap();
        write_file(&repo, "a.txt", b"Devin\n");
        let dir = TempDir::new().unwrap();

        let result = write_reverting(
            repo.path(),
            dir.path(),
            "a.txt",
            &found.staged,
            &found.local_hash,
        );

        assert!(matches!(result, Err(HunkError::Changed(_))));
        assert_eq!(fs::read(repo.path().join("a.txt")).unwrap(), b"Devin\n");
        assert!(snapshots::list(dir.path()).is_empty());
    }

    #[test]
    fn index_changed_outside_is_not_overwritten() {
        let repo = TempRepo::new();
        mixed(&repo);
        let found = three_sides(repo.path(), "a.txt").unwrap();
        repo.git(&["add", "a.txt"]);
        let devin = repo.git(&["show", ":a.txt"]);

        let result = write_index(repo.path(), "a.txt", "mine\n", &found.staged_hash);

        assert!(matches!(result, Err(HunkError::Changed(_))));
        assert_eq!(repo.git(&["show", ":a.txt"]), devin);
    }

    #[test]
    fn writing_the_index_rejects_foreign_paths_and_conflicts() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\n", "first");
        let hash = index_hash(&repo, "a.txt");

        let outside = write_index(repo.path(), "../a.txt", "x\n", &hash);
        let missing = write_index(repo.path(), "nothing.txt", "x\n", &hash);

        assert!(matches!(outside, Err(HunkError::OutsideRepository(_))));
        assert!(matches!(missing, Err(HunkError::NotEditable(_))));
    }
}
