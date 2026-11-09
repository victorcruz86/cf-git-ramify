use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::commands::ApiError;
use crate::commit::{self, PartialFile};
use crate::git::{self, GitError};
use crate::storage;

pub const SNAPSHOT_DAYS: u64 = 30;
pub const BEFORE_RESTORE: &str = "before restoring";

const SNAPSHOTS_DIR: &str = "snapshots";
const MANIFEST_FILE: &str = "manifest.json";
// The content is not stored under the repository path but under the number of its entry in the
// manifest: two paths that differ only in case (or in Unicode normalization) would
// overwrite each other on a volume that does not tell them apart, and one of the two files would be lost.
const FILES_DIR: &str = "files";
const STAGED_DIR: &str = "staged";
const PARTIAL_SUFFIX: &str = ".partial";
const RESTORE_TEMP_SUFFIX: &str = ".rama-restore";
const MILLIS_PER_DAY: u64 = 24 * 60 * 60 * 1000;

pub type SnapshotId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EntryKind {
    File,
    Symlink,
    // It did not exist on disk when copying: it is noted, but there is nothing to restore.
    Absent,
}

// Size and date of the file right before copying it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stamp {
    size: u64,
    modified_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub path: String,
    pub kind: EntryKind,
    // The index differed from HEAD when copying.
    pub staged: bool,
    // There was content in the index different from the disk and it was saved separately in `staged/`.
    pub staged_blob: bool,
    // Permissions of the file on disk. Old snapshots do not carry them.
    #[serde(default)]
    pub mode: Option<u32>,
    // Mode of the `staged/` entry (`100755`, `120000`), which need not be the disk's.
    #[serde(default)]
    pub staged_mode: Option<String>,
    #[serde(default)]
    pub stamp: Option<Stamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub id: SnapshotId,
    pub created_ms: u64,
    pub reason: String,
    pub entries: Vec<Entry>,
}

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn git_failure(error: GitError) -> io::Error {
    io::Error::other(ApiError::from(error).message)
}

// The path must be relative and without `..`, must not enter `.git` and must not leave the repository through a
// symbolic link in an intermediate folder: the real path of its folder must stay inside the
// repository and outside `.git`. The last component is not followed: a link is treated as a
// link, wherever it points (a committed `hooks -> .git/hooks` is copied, staged and committed
// as a link). Whoever writes through a final link must refuse it itself.
pub fn check_path(repo: &Path, path: &str) -> io::Result<()> {
    let relative = Path::new(path);
    let mut count = 0;
    for component in relative.components() {
        match component {
            Component::Normal(name) if !is_git_dir(name) => count += 1,
            _ => {
                return Err(invalid(format!(
                    "The path \"{path}\" is not valid inside the repository"
                )))
            }
        }
    }
    if count == 0 {
        return Err(invalid("The path is empty".to_string()));
    }

    let root = fs::canonicalize(repo)?;
    let into_git = || invalid(format!("The path \"{path}\" goes into .git"));
    let mut probe = repo.join(relative.parent().unwrap_or(Path::new("")));
    loop {
        match fs::canonicalize(&probe) {
            Ok(real) if in_git(&root, &real) => return Err(into_git()),
            Ok(real) if real.starts_with(&root) => return Ok(()),
            Ok(_) => {
                return Err(invalid(format!(
                    "The path \"{path}\" leaves the repository"
                )))
            }
            // A folder that is now a file does not exist for this path either.
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                // A broken link on the way is not crossed.
                if probe.symlink_metadata().is_ok() {
                    return Err(invalid(format!(
                        "The path \"{path}\" goes through a broken link"
                    )));
                }
                if !probe.pop() {
                    return Err(invalid(format!(
                        "The path \"{path}\" leaves the repository"
                    )));
                }
            }
            Err(error) => return Err(error),
        }
    }
}

fn is_git_dir(name: &std::ffi::OsStr) -> bool {
    name.eq_ignore_ascii_case(".git")
}

// Inside the repository's `.git` (or a nested one).
fn in_git(root: &Path, real: &Path) -> bool {
    real.strip_prefix(root).is_ok_and(|rest| {
        rest.components()
            .any(|part| matches!(part, Component::Normal(name) if is_git_dir(name)))
    })
}

fn changed_names(
    repo: &Path,
    head: &[&str],
    paths: &[String],
) -> Result<BTreeSet<String>, GitError> {
    let output = git::git_with_paths(repo, false, &git::exact(head), paths)?;
    Ok(output
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
        .map(|name| String::from_utf8_lossy(name).into_owned())
        .collect())
}

struct StagedInfo {
    staged: BTreeSet<String>,
    // With changes in the index and also different from the disk: that content would be lost.
    differs_from_disk: BTreeSet<String>,
}

fn staged_info(repo: &Path, paths: &[String]) -> Result<StagedInfo, GitError> {
    let diff = ["diff", "--no-renames", "--no-ext-diff", "--name-only", "-z"];
    let cached = [
        "diff",
        "--cached",
        "--no-renames",
        "--no-ext-diff",
        "--name-only",
        "-z",
    ];
    let staged = changed_names(repo, &cached, paths)?;
    let on_disk = changed_names(repo, &diff, paths)?;
    let differs_from_disk = staged.intersection(&on_disk).cloned().collect();
    Ok(StagedInfo {
        staged,
        differs_from_disk,
    })
}

// Paths whose stage differs from HEAD and from the disk: that content exists only in the index.
pub fn staged_only(repo: &Path, paths: &[String]) -> Result<BTreeSet<String>, GitError> {
    Ok(staged_info(repo, paths)?.differs_from_disk)
}

// Entries `mode hash stage<TAB>path`: only stage 0 of that exact path counts.
fn stage_zero_mode(repo: &Path, path: &str) -> Result<Option<String>, GitError> {
    let head = git::exact(&["ls-files", "-z", "-s"]);
    let output = git::git_with_paths(repo, false, &head, &[path.to_string()])?;
    Ok(output.split(|byte| *byte == 0).find_map(|entry| {
        let mut parts = entry.splitn(2, |byte| *byte == b'\t');
        let (header, name) = (parts.next()?, parts.next()?);
        let header = String::from_utf8_lossy(header);
        let mode = header.split(' ').next()?;
        (name == path.as_bytes() && header.ends_with(" 0")).then(|| mode.to_string())
    }))
}

fn has_stage_zero(repo: &Path, path: &str) -> Result<bool, GitError> {
    Ok(stage_zero_mode(repo, path)?.is_some())
}

// `:0:` is stage 0 of the index. Without it (conflict) there is no content to save; any
// other failure is propagated so the snapshot is not published without the stage's content. It also returns
// the entry's mode.
fn staged_blob(repo: &Path, path: &str) -> Result<Option<(String, Vec<u8>)>, GitError> {
    let Some(mode) = stage_zero_mode(repo, path)? else {
        return Ok(None);
    };
    let spec = format!(":0:{path}");
    let bytes = git::run(repo, &git::exact(&["show", "--no-textconv", &spec]))?;
    Ok(Some((mode, bytes)))
}

fn now_millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

// Sorts by time; the counter breaks ties within the same millisecond and the process id across processes.
fn new_id(now: SystemTime) -> SnapshotId {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!(
        "{:016}-{:012}-{}",
        now_millis(now),
        COUNTER.fetch_add(1, Ordering::Relaxed),
        process::id()
    )
}

fn check_id(id: &str) -> io::Result<()> {
    let valid = !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    if valid {
        Ok(())
    } else {
        Err(invalid(format!("Invalid snapshot identifier: \"{id}\"")))
    }
}

fn snapshots_root(dir: &Path) -> PathBuf {
    dir.join(SNAPSHOTS_DIR)
}

fn slot(folder: &Path, staged: bool, index: usize) -> PathBuf {
    let area = if staged { STAGED_DIR } else { FILES_DIR };
    folder.join(area).join(index.to_string())
}

// Where the content of a path ended up inside a snapshot (`staged`: the index blob).
pub fn stored(dir: &Path, id: &str, path: &str, staged: bool) -> Option<PathBuf> {
    check_id(id).ok()?;
    let folder = snapshots_root(dir).join(id);
    let manifest = read_manifest(&folder).ok()?;
    let index = manifest
        .entries
        .iter()
        .position(|entry| entry.path == path)?;
    Some(slot(&folder, staged, index))
}

// Where each file of a snapshot is stored, from one read of its manifest.
pub fn stored_files(dir: &Path, id: &str) -> io::Result<BTreeMap<String, PathBuf>> {
    check_id(id)?;
    let folder = snapshots_root(dir).join(id);
    let manifest = read_manifest(&folder)?;
    Ok(manifest
        .entries
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.path.clone(), slot(&folder, false, index)))
        .collect())
}

#[cfg(unix)]
fn make_symlink(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(not(unix))]
fn make_symlink(_target: &Path, _link: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Symbolic links are only supported on Unix systems",
    ))
}

#[cfg(unix)]
fn file_mode(meta: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(meta.permissions().mode() & 0o7777)
}

#[cfg(not(unix))]
fn file_mode(_meta: &fs::Metadata) -> Option<u32> {
    None
}

#[cfg(unix)]
fn apply_mode(path: &Path, mode: Option<u32>) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    mode.map_or(Ok(()), |mode| {
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
    })
}

#[cfg(not(unix))]
fn apply_mode(_path: &Path, _mode: Option<u32>) -> io::Result<()> {
    Ok(())
}

fn sync_dir(path: &Path) {
    let _ = fs::File::open(path).and_then(|dir| dir.sync_all());
}

fn stamp_of(meta: &fs::Metadata) -> io::Result<Stamp> {
    let modified = meta.modified()?.duration_since(UNIX_EPOCH);
    Ok(Stamp {
        size: meta.len(),
        modified_ns: modified.map_or(0, |elapsed| elapsed.as_nanos() as u64),
    })
}

// What a snapshot knows about the file on disk. A link's permissions mean nothing.
struct Copied {
    kind: EntryKind,
    mode: Option<u32>,
    stamp: Option<Stamp>,
}

impl Copied {
    const ABSENT: Self = Self {
        kind: EntryKind::Absent,
        mode: None,
        stamp: None,
    };
}

fn copy_entry(repo: &Path, target: &Path, path: &str) -> io::Result<Copied> {
    let source = repo.join(path);
    let meta = match source.symlink_metadata() {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Copied::ABSENT),
        Err(error) => return Err(error),
    };
    let kind = meta.file_type();
    let stamp = Some(stamp_of(&meta)?);
    if kind.is_symlink() {
        make_symlink(&fs::read_link(&source)?, target)?;
        Ok(Copied {
            kind: EntryKind::Symlink,
            mode: None,
            stamp,
        })
    } else if kind.is_file() {
        fs::copy(&source, target)?;
        fs::File::open(target)?.sync_all()?;
        Ok(Copied {
            kind: EntryKind::File,
            mode: file_mode(&meta),
            stamp,
        })
    } else if kind.is_dir() {
        Err(invalid(format!(
            "\"{path}\" is a folder: it cannot be copied as a file"
        )))
    } else {
        Err(invalid(format!("\"{path}\" is not a regular file")))
    }
}

fn fill(
    partial: &Path,
    repo: &Path,
    paths: &BTreeSet<String>,
    info: &StagedInfo,
    (id, created_ms, reason): (SnapshotId, u64, &str),
    from_index: bool,
) -> io::Result<()> {
    fs::create_dir(partial.join(FILES_DIR))?;
    fs::create_dir(partial.join(STAGED_DIR))?;
    let mut entries = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        let only_index = from_index
            && displaced(repo, path)
            && has_stage_zero(repo, path).map_err(git_failure)?;
        let copied = if only_index {
            Copied::ABSENT
        } else {
            copy_entry(repo, &slot(partial, false, index), path)?
        };
        let mut staged_mode = None;
        if only_index || info.differs_from_disk.contains(path) {
            if let Some((mode, bytes)) = staged_blob(repo, path).map_err(git_failure)? {
                storage::write_atomic(&slot(partial, true, index), &bytes)?;
                staged_mode = Some(mode);
            }
        }
        entries.push(Entry {
            path: path.clone(),
            kind: copied.kind,
            staged: info.staged.contains(path),
            staged_blob: staged_mode.is_some(),
            mode: copied.mode,
            staged_mode,
            stamp: copied.stamp,
        });
    }
    let manifest = Snapshot {
        id,
        created_ms,
        reason: reason.to_string(),
        entries,
    };
    let json = serde_json::to_vec_pretty(&manifest).map_err(io::Error::other)?;
    storage::write_atomic(&partial.join(MANIFEST_FILE), &json)
}

// The snapshot is assembled in a hidden folder and is only published under its final name when it is
// complete on disk: whoever lists or restores never sees a half-done snapshot.
pub fn create(dir: &Path, repo: &Path, paths: &[String], reason: &str) -> io::Result<SnapshotId> {
    create_at(dir, repo, paths, reason, SystemTime::now())
}

// For what only changes the index: the disk is not touched, so what cannot be copied from it is not
// a reason to stop. For a displaced path, the content of its entry is kept. `create`, in
// contrast, fails: whoever is going to change the disk must not continue without a copy of what it will overwrite.
pub fn create_for_index(
    dir: &Path,
    repo: &Path,
    paths: &[String],
    reason: &str,
) -> io::Result<SnapshotId> {
    build(dir, repo, paths, reason, SystemTime::now(), true)
}

// One of the folders of `path` no longer is one: it is now a file, or a link. Each
// component is checked: following the link would lead somewhere else, and there the path would look free.
pub fn behind_other(repo: &Path, path: &str) -> bool {
    let mut current = repo.to_path_buf();
    let mut parts = Path::new(path).components().peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            return false;
        }
        current.push(part);
        match current.symlink_metadata() {
            Ok(meta) if !meta.is_dir() => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
    false
}

// In the place of `path` there is now a folder, or one of its folders is no longer one.
pub fn displaced(repo: &Path, path: &str) -> bool {
    behind_other(repo, path)
        || repo
            .join(path)
            .symlink_metadata()
            .is_ok_and(|meta| meta.is_dir())
}

// The stage 0 entries that cannot coexist in the index with these paths: what is below
// them and the files in their way. The index is read once: `Stage All` passes many paths.
pub fn index_clashes(repo: &Path, paths: &[String]) -> Result<BTreeSet<String>, GitError> {
    let output = git::run(repo, &git::exact(&["ls-files", "-z", "-s"]))?;
    let names: BTreeSet<String> = output
        .split(|byte| *byte == 0)
        .filter_map(|entry| {
            let mut parts = entry.splitn(2, |byte| *byte == b'\t');
            let (header, name) = (parts.next()?, parts.next()?);
            header
                .ends_with(b" 0")
                .then(|| String::from_utf8_lossy(name).into_owned())
        })
        .collect();
    let mut clashes = BTreeSet::new();
    for path in paths {
        let below = format!("{path}/");
        let under = names.range(below.clone()..);
        clashes.extend(under.take_while(|name| name.starts_with(&below)).cloned());
        for (position, _) in path.match_indices('/') {
            if let Some(name) = names.get(&path[..position]) {
                clashes.insert(name.clone());
            }
        }
    }
    Ok(clashes)
}

// What is versioned with changes, in the index or on disk. A submodule is not included: reset and checkout do not enter it.
fn tracked_changes(repo: &Path) -> Result<Vec<String>, GitError> {
    let output = git::run(
        repo,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=no",
            "--no-renames",
            "--ignore-submodules=all",
        ],
    )?;
    Ok(output
        .split(|byte| *byte == 0)
        .filter_map(|entry| entry.get(3..))
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .collect())
}

struct Indexed {
    names: BTreeSet<String>,
    // Marked `assume-unchanged` or `skip-worktree`: `status` does not show them even if they changed.
    hidden: Vec<String>,
}

// `ls-files -v` gives a tag in front: lowercase for `assume-unchanged` and `S` for the other.
fn read_index(repo: &Path) -> Result<Indexed, GitError> {
    let output = git::run(repo, &["ls-files", "-z", "-v"])?;
    let mut indexed = Indexed {
        names: BTreeSet::new(),
        hidden: Vec::new(),
    };
    for entry in output.split(|byte| *byte == 0) {
        let (Some(tag), Some(name)) = (entry.first(), entry.get(2..)) else {
            continue;
        };
        let name = String::from_utf8_lossy(name).into_owned();
        if tag.is_ascii_lowercase() || *tag == b'S' {
            indexed.hidden.push(name.clone());
        }
        indexed.names.insert(name);
    }
    Ok(indexed)
}

// The first thing on the way of `path` that is not a folder.
fn blocked_by(repo: &Path, path: &str) -> Option<String> {
    path.match_indices('/')
        .map(|(position, _)| &path[..position])
        .find(|prefix| {
            let meta = repo.join(prefix).symlink_metadata();
            meta.is_ok_and(|meta| !meta.is_dir())
        })
        .map(str::to_string)
}

// What is on disk at that path and `git` would delete to put something else: a file, or everything
// loose in a folder. What is in the index is recovered from it, and if it has changes it already counts.
fn add_on_disk(
    repo: &Path,
    path: &str,
    indexed: &BTreeSet<String>,
    found: &mut BTreeSet<String>,
) -> io::Result<()> {
    let meta = match repo.join(path).symlink_metadata() {
        Ok(meta) => meta,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(())
        }
        Err(error) => return Err(error),
    };
    if !meta.is_dir() {
        found.insert(path.to_string());
        return Ok(());
    }
    for entry in fs::read_dir(repo.join(path))? {
        let name = entry?.file_name();
        // A name that is not UTF-8 would not reach the snapshot whole: better to touch nothing.
        let name = name.to_str().ok_or_else(|| {
            invalid(format!(
                "There is a name in \"{path}\" that cannot be copied: nothing has been touched"
            ))
        })?;
        let child = format!("{path}/{name}");
        if !indexed.contains(&child) {
            add_on_disk(repo, &child, indexed, found)?;
        }
    }
    Ok(())
}

// What is unversioned (ignored or not) that occupies a place of `commit`.
fn add_in_the_way(
    repo: &Path,
    commit: &str,
    indexed: &BTreeSet<String>,
    found: &mut BTreeSet<String>,
) -> Result<(), GitError> {
    let tree = git::run(repo, &["ls-tree", "-r", "-z", commit])?;
    for entry in tree.split(|byte| *byte == 0) {
        let mut parts = entry.splitn(2, |byte| *byte == b'\t');
        let (Some(header), Some(name)) = (parts.next(), parts.next()) else {
            continue;
        };
        let name = String::from_utf8_lossy(name);
        // A submodule is a folder that `git` leaves where it is.
        if header.starts_with(b"160000") || indexed.contains(name.as_ref()) {
            continue;
        }
        if let Some(prefix) = blocked_by(repo, &name) {
            found.insert(prefix);
        } else {
            add_on_disk(repo, &name, indexed, found).map_err(git::io_error)?;
        }
    }
    Ok(())
}

// What a normal checkout overwrites without warning: `git` only refuses with what is unversioned and not
// ignored.
pub fn in_the_way(repo: &Path, commit: &str) -> Result<Vec<String>, GitError> {
    let indexed = read_index(repo)?;
    let mut found = BTreeSet::new();
    add_in_the_way(repo, commit, &indexed.names, &mut found)?;
    Ok(found.into_iter().collect())
}

// What `reset --hard` and `checkout --force` to `commit` would lose: the changes, and what is unversioned
// that occupies the place of something in the target. For them `.gitignore` does not exist either, and a loose
// folder in the place of a file goes away whole.
pub fn at_risk(repo: &Path, commit: &str) -> Result<Vec<String>, GitError> {
    let indexed = read_index(repo)?;
    let mut found = BTreeSet::new();
    for path in tracked_changes(repo)? {
        if let Some(prefix) = blocked_by(repo, &path) {
            found.insert(prefix);
        } else if repo
            .join(&path)
            .symlink_metadata()
            .is_ok_and(|meta| meta.is_dir())
        {
            add_on_disk(repo, &path, &indexed.names, &mut found).map_err(git::io_error)?;
        } else {
            found.insert(path);
        }
    }
    for path in &indexed.hidden {
        add_on_disk(repo, path, &BTreeSet::new(), &mut found).map_err(git::io_error)?;
    }
    add_in_the_way(repo, commit, &indexed.names, &mut found)?;
    Ok(found.into_iter().collect())
}

// What exists only in the index: reset `--mixed` and `--keep` leave the disk, but not the stage.
pub fn index_only(repo: &Path) -> Result<Vec<String>, GitError> {
    Ok(staged_only(repo, &tracked_changes(repo)?)?
        .into_iter()
        .collect())
}

fn create_at(
    dir: &Path,
    repo: &Path,
    paths: &[String],
    reason: &str,
    now: SystemTime,
) -> io::Result<SnapshotId> {
    build(dir, repo, paths, reason, now, false)
}

fn build(
    dir: &Path,
    repo: &Path,
    paths: &[String],
    reason: &str,
    now: SystemTime,
    from_index: bool,
) -> io::Result<SnapshotId> {
    let paths: BTreeSet<String> = paths.iter().cloned().collect();
    for path in &paths {
        check_path(repo, path)?;
    }
    let ordered: Vec<String> = paths.iter().cloned().collect();
    let info = staged_info(repo, &ordered).map_err(git_failure)?;

    let root = snapshots_root(dir);
    fs::create_dir_all(&root)?;
    let id = new_id(now);
    let partial = root.join(format!(".{id}{PARTIAL_SUFFIX}"));
    fs::create_dir(&partial)?;
    let header = (id.clone(), now_millis(now), reason);
    let result = fill(&partial, repo, &paths, &info, header, from_index)
        .and_then(|()| fs::rename(&partial, root.join(&id)));
    if let Err(error) = result {
        let _ = fs::remove_dir_all(&partial);
        return Err(error);
    }
    sync_dir(&root);
    Ok(id)
}

fn stamp_on_disk(repo: &Path, path: &str) -> io::Result<Option<Stamp>> {
    match repo.join(path).symlink_metadata() {
        Ok(meta) => stamp_of(&meta).map(Some),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

// The paths that are no longer what they were when copied: what was written afterwards is not in the snapshot. What did not
// exist must still not exist, and a path the snapshot does not know counts as changed.
pub fn changed_since(
    dir: &Path,
    repo: &Path,
    id: &str,
    paths: &[String],
) -> io::Result<Vec<String>> {
    check_id(id)?;
    let snapshot = read_manifest(&snapshots_root(dir).join(id))?;
    let stamps: BTreeMap<&str, Option<Stamp>> = snapshot
        .entries
        .iter()
        .map(|entry| (entry.path.as_str(), entry.stamp))
        .collect();
    let mut changed = Vec::new();
    for path in paths {
        if stamps.get(path.as_str()) != Some(&stamp_on_disk(repo, path)?) {
            changed.push(path.clone());
        }
    }
    Ok(changed)
}

fn read_manifest(folder: &Path) -> io::Result<Snapshot> {
    let bytes = fs::read(folder.join(MANIFEST_FILE))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

// From newest to oldest. Unreadable or incomplete folders do not appear.
pub fn list(dir: &Path) -> Vec<Snapshot> {
    let Ok(read) = fs::read_dir(snapshots_root(dir)) else {
        return Vec::new();
    };
    let mut snapshots: Vec<Snapshot> = read
        .filter_map(Result::ok)
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|entry| read_manifest(&entry.path()).ok())
        .collect();
    snapshots.sort_by(|a, b| b.id.cmp(&a.id));
    snapshots
}

fn restore_entry(folder: &Path, repo: &Path, index: usize, entry: &Entry) -> io::Result<()> {
    let source = slot(folder, false, index);
    let target = repo.join(&entry.path);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut temp_name = OsString::from(".");
    temp_name.push(target.file_name().ok_or(io::ErrorKind::InvalidInput)?);
    temp_name.push(RESTORE_TEMP_SUFFIX);
    let temp = target.with_file_name(temp_name);
    let _ = fs::remove_file(&temp);

    let written = match entry.kind {
        EntryKind::Symlink => fs::read_link(&source).and_then(|link| make_symlink(&link, &temp)),
        _ => fs::copy(&source, &temp).and_then(|_| apply_mode(&temp, entry.mode)),
    };
    let result = written.and_then(|()| fs::rename(&temp, &target));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

// What was only in the index comes back with the same bytes, without filters, and with its mode. An old
// snapshot does not carry it: it keeps the one from the current entry.
fn restore_staged(folder: &Path, repo: &Path, index: usize, entry: &Entry) -> io::Result<()> {
    let file = PartialFile {
        path: entry.path.clone(),
        content: fs::read(slot(folder, true, index))?,
    };
    if let Some(mode) = &entry.staged_mode {
        return commit::write_entry(repo, &file, mode).map_err(git_failure);
    }
    let saved = commit::read_entries(repo, std::slice::from_ref(&entry.path));
    saved
        .and_then(|saved| commit::write_partial(repo, &[file], &saved))
        .map_err(git_failure)
}

// Returns the id of the snapshot taken of the current state before writing: this way a
// restore never silently overwrites newer edits. If that snapshot fails, nothing is touched.
pub fn restore(dir: &Path, repo: &Path, id: &str) -> io::Result<SnapshotId> {
    check_id(id)?;
    let folder = snapshots_root(dir).join(id);
    let snapshot = read_manifest(&folder)?;
    let entries: Vec<(usize, &Entry)> = snapshot
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.kind != EntryKind::Absent)
        .collect();
    for (index, entry) in &entries {
        check_path(repo, &entry.path)?;
        if displaced(repo, &entry.path) {
            return Err(invalid(format!(
                "There is now something else on disk in the place of \"{}\": move it before restoring. Nothing has been touched",
                entry.path
            )));
        }
        let stored = slot(&folder, false, *index).symlink_metadata()?;
        let expected = match entry.kind {
            EntryKind::Symlink => stored.file_type().is_symlink(),
            _ => stored.file_type().is_file(),
        };
        if !expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("The snapshot of \"{}\" is damaged", entry.path),
            ));
        }
    }

    let staged: Vec<(usize, &Entry)> = snapshot
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.staged_blob)
        .collect();
    for (index, entry) in &staged {
        check_path(repo, &entry.path)?;
        let stored = slot(&folder, true, *index).symlink_metadata();
        if !stored.is_ok_and(|meta| meta.file_type().is_file()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("The snapshot of \"{}\" is damaged", entry.path),
            ));
        }
    }
    // Putting an entry in the index would delete the conflict stages, and the merge would consider it
    // resolved with the snapshot's content.
    let in_index: Vec<String> = staged.iter().map(|(_, e)| e.path.clone()).collect();
    if let Some(path) = commit::conflicted(repo, &in_index)
        .map_err(git_failure)?
        .first()
    {
        return Err(invalid(format!(
            "\"{path}\" has unresolved conflicts: resolve them before restoring. Nothing has been touched"
        )));
    }

    // An entry that clashes with another would make `update-index` fail halfway: it is reported beforehand, and what
    // is in the stage stays.
    let clashes = index_clashes(repo, &in_index).map_err(git_failure)?;
    if let Some(clash) = clashes.first() {
        return Err(invalid(format!(
            "The saved content cannot be put back into the stage because it clashes with \"{clash}\", which is already in it. The content remains in the snapshot and nothing has been touched"
        )));
    }

    // For a displaced path that is no longer in the index there is nothing to snapshot.
    let mut paths = Vec::new();
    for entry in &snapshot.entries {
        let in_place = entry.kind != EntryKind::Absent
            || (entry.staged_blob
                && (!displaced(repo, &entry.path)
                    || has_stage_zero(repo, &entry.path).map_err(git_failure)?));
        if in_place {
            paths.push(entry.path.clone());
        }
    }
    let backup = create_for_index(dir, repo, &paths, BEFORE_RESTORE)?;
    for (index, entry) in entries {
        restore_entry(&folder, repo, index, entry)?;
    }
    for (index, entry) in staged {
        restore_staged(&folder, repo, index, entry)?;
    }
    Ok(backup)
}

// Removes snapshots older than `SNAPSHOT_DAYS` days and the remains of snapshots that were never
// finished. It tries all of them and returns how many it removed, or the first error if any failed.
pub fn prune(dir: &Path, now: SystemTime) -> io::Result<usize> {
    let read = match fs::read_dir(snapshots_root(dir)) {
        Ok(read) => read,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let cutoff = now_millis(now).saturating_sub(SNAPSHOT_DAYS * MILLIS_PER_DAY);
    let mut removed = 0;
    let mut first_error = None;
    for entry in read.filter_map(Result::ok) {
        let is_folder = entry.file_type().is_ok_and(|kind| kind.is_dir());
        if !is_folder || !is_expired(&entry, cutoff) {
            continue;
        }
        match fs::remove_dir_all(entry.path()) {
            Ok(()) => removed += 1,
            Err(error) => first_error = first_error.or(Some(error)),
        }
    }
    first_error.map_or(Ok(removed), Err)
}

// Without a readable manifest it is judged by the folder's date.
fn is_expired(entry: &fs::DirEntry, cutoff: u64) -> bool {
    let hidden = entry.file_name().to_string_lossy().starts_with('.');
    if !hidden {
        if let Ok(snapshot) = read_manifest(&entry.path()) {
            return snapshot.created_ms < cutoff;
        }
    }
    entry
        .metadata()
        .and_then(|meta| meta.modified())
        .is_ok_and(|modified| now_millis(modified) < cutoff)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempRepo;
    use std::collections::BTreeSet;
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::time::Duration;
    use tempfile::TempDir;

    fn paths(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn write(repo: &TempRepo, file: &str, contents: &[u8]) {
        let target = repo.path().join(file);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, contents).unwrap();
    }

    fn read(repo: &TempRepo, file: &str) -> Vec<u8> {
        fs::read(repo.path().join(file)).unwrap()
    }

    fn chmod(repo: &TempRepo, file: &str, mode: u32) {
        let target = repo.path().join(file);
        fs::set_permissions(target, fs::Permissions::from_mode(mode)).unwrap();
    }

    fn mode_on_disk(repo: &TempRepo, file: &str) -> u32 {
        let meta = repo.path().join(file).symlink_metadata().unwrap();
        meta.permissions().mode() & 0o777
    }

    fn mode_in_index(repo: &TempRepo, file: &str) -> String {
        let line = repo.git(&["ls-files", "-s", "--", file]);
        line.split(' ').next().unwrap().to_string()
    }

    fn entry_of(dir: &TempDir, file: &str) -> Entry {
        let entries = list(dir.path()).remove(0).entries;
        entries
            .into_iter()
            .find(|entry| entry.path == file)
            .unwrap()
    }

    #[test]
    fn restore_returns_the_permissions_to_the_file() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.sh", b"1\n", "one");
        write(&repo, "a.sh", b"2\n");
        chmod(&repo, "a.sh", 0o755);
        let id = create(dir.path(), repo.path(), &paths(&["a.sh"]), "test").unwrap();
        assert_eq!(entry_of(&dir, "a.sh").mode, Some(0o755));
        // The permissions come from the manifest, not from whatever happens to the snapshot's file.
        let copy = stored(dir.path(), &id, "a.sh", false).unwrap();
        fs::set_permissions(copy, fs::Permissions::from_mode(0o644)).unwrap();
        chmod(&repo, "a.sh", 0o644);

        restore(dir.path(), repo.path(), &id).unwrap();

        assert_eq!(mode_on_disk(&repo, "a.sh"), 0o755);
    }

    #[test]
    fn restore_returns_the_mode_to_the_stage() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.sh", b"1\n", "one");
        write(&repo, "a.sh", b"in stage\n");
        chmod(&repo, "a.sh", 0o755);
        repo.git(&["add", "a.sh"]);
        write(&repo, "a.sh", b"on disk\n");
        chmod(&repo, "a.sh", 0o644);
        let id = create(dir.path(), repo.path(), &paths(&["a.sh"]), "test").unwrap();
        repo.git(&["reset", "-q", "--hard"]);

        restore(dir.path(), repo.path(), &id).unwrap();

        assert_eq!(mode_in_index(&repo, "a.sh"), "100755");
        assert_eq!(mode_on_disk(&repo, "a.sh"), 0o644);
        assert_eq!(repo.git(&["show", ":a.sh"]), "in stage\n");
    }

    // `AD`: without a file on disk, the mode can only come from the snapshot.
    #[test]
    fn restore_returns_the_mode_of_an_executable_only_in_stage() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1\n", "one");
        write(&repo, "new.sh", b"in stage\n");
        chmod(&repo, "new.sh", 0o755);
        repo.git(&["add", "new.sh"]);
        fs::remove_file(repo.path().join("new.sh")).unwrap();
        let id = create(dir.path(), repo.path(), &paths(&["new.sh"]), "test").unwrap();
        repo.git(&["rm", "-q", "--cached", "new.sh"]);

        restore(dir.path(), repo.path(), &id).unwrap();

        assert_eq!(mode_in_index(&repo, "new.sh"), "100755");
        assert_eq!(repo.git(&["show", ":new.sh"]), "in stage\n");
    }

    #[test]
    fn restore_returns_a_link_from_the_stage_as_a_link() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1\n", "one");
        symlink("a.txt", repo.path().join("link")).unwrap();
        repo.git(&["add", "link"]);
        fs::remove_file(repo.path().join("link")).unwrap();
        symlink("other.txt", repo.path().join("link")).unwrap();
        let id = create(dir.path(), repo.path(), &paths(&["link"]), "test").unwrap();
        repo.git(&["rm", "-q", "-f", "--cached", "link"]);
        fs::remove_file(repo.path().join("link")).unwrap();

        restore(dir.path(), repo.path(), &id).unwrap();

        assert_eq!(mode_in_index(&repo, "link"), "120000");
        assert_eq!(repo.git(&["show", ":link"]), "a.txt");
        let link = fs::read_link(repo.path().join("link")).unwrap();
        assert_eq!(link, std::path::Path::new("other.txt"));
    }

    #[test]
    fn an_old_snapshot_without_modes_is_read_and_restored() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1\n", "one");
        write(&repo, "a.txt", b"in stage\n");
        repo.git(&["add", "a.txt"]);
        write(&repo, "a.txt", b"on disk\n");
        let id = create(dir.path(), repo.path(), &paths(&["a.txt"]), "test").unwrap();
        let manifest = snapshots_root(dir.path()).join(&id).join(MANIFEST_FILE);
        let mut json: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
        for entry in json["entries"].as_array_mut().unwrap() {
            let entry = entry.as_object_mut().unwrap();
            entry.remove("mode");
            entry.remove("stagedMode");
        }
        fs::write(&manifest, serde_json::to_vec(&json).unwrap()).unwrap();
        repo.git(&["reset", "-q", "--hard"]);

        assert_eq!(entry_of(&dir, "a.txt").staged_mode, None);
        restore(dir.path(), repo.path(), &id).unwrap();

        assert_eq!(read(&repo, "a.txt"), b"on disk\n");
        assert_eq!(repo.git(&["show", ":a.txt"]), "in stage\n");
    }

    #[test]
    fn binary_file() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        let bytes: Vec<u8> = (0..=255).chain([13, 10, 0, 0, 255]).collect();
        repo.commit("data.bin", b"\x00\x01", "one");
        write(&repo, "data.bin", &bytes);

        let id = create(dir.path(), repo.path(), &paths(&["data.bin"]), "test").unwrap();

        assert_eq!(
            fs::read(stored(dir.path(), &id, "data.bin", false).unwrap()).unwrap(),
            bytes
        );
        write(&repo, "data.bin", b"something else");
        restore(dir.path(), repo.path(), &id).unwrap();
        assert_eq!(read(&repo, "data.bin"), bytes);
    }

    #[test]
    fn symlink_is_copied_as_a_link() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let secret = outside.path().join("secret.txt");
        fs::write(&secret, "secret").unwrap();
        symlink(&secret, repo.path().join("link")).unwrap();

        let id = create(dir.path(), repo.path(), &paths(&["link"]), "test").unwrap();

        let copy = stored(dir.path(), &id, "link", false).unwrap();
        assert!(copy.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(fs::read_link(&copy).unwrap(), secret);
        let snapshot = list(dir.path()).remove(0);
        assert_eq!(snapshot.entries[0].kind, EntryKind::Symlink);

        fs::remove_file(repo.path().join("link")).unwrap();
        restore(dir.path(), repo.path(), &id).unwrap();
        let restored = repo.path().join("link");
        assert!(restored
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_link(restored).unwrap(), secret);
        assert_eq!(fs::read(&secret).unwrap(), b"secret");
    }

    // A link is copied as a link, wherever it points: a committed `hooks -> .git/hooks` is the
    // user's file like any other.
    #[test]
    fn a_link_into_git_is_kept_as_a_link() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        symlink(".git/hooks", repo.path().join("hooks")).unwrap();
        repo.git(&["add", "hooks"]);
        repo.git(&["commit", "-q", "-m", "link"]);

        check_path(repo.path(), "hooks").unwrap();
        let id = create(dir.path(), repo.path(), &paths(&["hooks"]), "test").unwrap();
        let copy = stored(dir.path(), &id, "hooks", false).unwrap();
        assert_eq!(fs::read_link(copy).unwrap(), Path::new(".git/hooks"));
        assert_eq!(list(dir.path())[0].entries[0].kind, EntryKind::Symlink);

        // Staged through the usual helper.
        fs::remove_file(repo.path().join("hooks")).unwrap();
        symlink(".git/info", repo.path().join("hooks")).unwrap();
        crate::staging::stage(repo.path(), dir.path(), &paths(&["hooks"])).unwrap();
        assert_eq!(repo.git(&["diff", "--cached", "--name-only"]), "hooks\n");
    }

    #[test]
    fn paths_outside_the_repository_are_rejected() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("x.txt"), "outside").unwrap();
        symlink(outside.path(), repo.path().join("outlink")).unwrap();
        // A link to `.git`, and a link that is itself a file of `.git`.
        symlink(".git", repo.path().join("gitlink")).unwrap();
        fs::create_dir(repo.path().join("sub")).unwrap();

        let invalid = [
            "../x.txt",
            "sub/../../x.txt",
            "/etc/passwd",
            "",
            "outlink/x.txt",
            "outlink/new/x.txt",
            ".git/config",
            "sub/.git/config",
            "gitlink/config",
            "gitlink/hooks/new",
        ];
        for path in invalid {
            let error = check_path(repo.path(), path).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{path:?}");
            assert!(
                create(dir.path(), repo.path(), &paths(&[path]), "test").is_err(),
                "{path:?}"
            );
        }
        assert!(list(dir.path()).is_empty());

        for path in ["a.txt", "sub/new/b.txt", "outlink"] {
            check_path(repo.path(), path).unwrap();
        }
    }

    #[test]
    fn snapshot_saves_the_stage_different_from_the_disk() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1", "one");
        repo.commit("b.txt", b"1", "two");
        write(&repo, "a.txt", b"2");
        write(&repo, "b.txt", b"2");
        repo.git(&["add", "a.txt", "b.txt"]);
        write(&repo, "a.txt", b"3");

        let id = create(dir.path(), repo.path(), &paths(&["a.txt", "b.txt"]), "test").unwrap();

        assert_eq!(
            fs::read(stored(dir.path(), &id, "a.txt", false).unwrap()).unwrap(),
            b"3"
        );
        assert_eq!(
            fs::read(stored(dir.path(), &id, "a.txt", true).unwrap()).unwrap(),
            b"2"
        );
        assert!(!stored(dir.path(), &id, "b.txt", true).unwrap().exists());
        let entries = list(dir.path()).remove(0).entries;
        assert_eq!((entries[0].staged, entries[0].staged_blob), (true, true));
        assert_eq!((entries[1].staged, entries[1].staged_blob), (true, false));
    }

    #[test]
    fn snapshot_of_file_no_longer_on_disk() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1", "one");
        fs::remove_file(repo.path().join("a.txt")).unwrap();

        let id = create(dir.path(), repo.path(), &paths(&["a.txt"]), "test").unwrap();

        assert!(!stored(dir.path(), &id, "a.txt", false).unwrap().exists());
        assert_eq!(list(dir.path())[0].entries[0].kind, EntryKind::Absent);
    }

    #[test]
    fn restore_returns_what_was_only_in_the_stage() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("run.sh", b"base\n", "base");
        let exec = fs::Permissions::from_mode(0o755);
        fs::set_permissions(repo.path().join("run.sh"), exec).unwrap();
        repo.git(&["add", "run.sh"]);
        repo.git(&["commit", "-q", "-m", "executable"]);
        fs::write(repo.path().join("run.sh"), b"in stage\n").unwrap();
        repo.git(&["add", "run.sh"]);
        fs::write(repo.path().join("run.sh"), b"on disk\n").unwrap();
        let id = create(dir.path(), repo.path(), &["run.sh".to_string()], "test").unwrap();
        repo.git(&["add", "run.sh"]);
        fs::write(repo.path().join("run.sh"), b"other\n").unwrap();

        restore(dir.path(), repo.path(), &id).unwrap();

        assert_eq!(repo.git(&["show", ":run.sh"]), "in stage\n");
        assert_eq!(fs::read(repo.path().join("run.sh")).unwrap(), b"on disk\n");
        assert!(repo
            .git(&["ls-files", "-s", "run.sh"])
            .starts_with("100755 "));
    }

    #[test]
    fn restore_returns_the_stage_of_a_file_that_was_not_on_disk() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1\n", "base");
        write(&repo, "new.txt", b"in stage\n");
        repo.git(&["add", "new.txt"]);
        fs::remove_file(repo.path().join("new.txt")).unwrap();
        let id = create(dir.path(), repo.path(), &paths(&["new.txt"]), "test").unwrap();
        repo.git(&["rm", "-q", "--cached", "new.txt"]);

        restore(dir.path(), repo.path(), &id).unwrap();

        assert_eq!(repo.git(&["show", ":new.txt"]), "in stage\n");
        assert!(!repo.path().join("new.txt").exists());
    }

    #[test]
    fn restore_first_takes_a_snapshot_of_the_current_state() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1", "one");
        write(&repo, "a.txt", b"2");
        let id = create(dir.path(), repo.path(), &paths(&["a.txt"]), "test").unwrap();
        write(&repo, "a.txt", "newer edit".as_bytes());

        let backup = restore(dir.path(), repo.path(), &id).unwrap();

        assert_eq!(read(&repo, "a.txt"), b"2");
        assert_ne!(backup, id);
        let listed = list(dir.path());
        let saved = listed
            .iter()
            .find(|snapshot| snapshot.id == backup)
            .unwrap();
        assert_eq!(saved.reason, BEFORE_RESTORE);
        assert_eq!(
            fs::read(stored(dir.path(), &backup, "a.txt", false).unwrap()).unwrap(),
            "newer edit".as_bytes()
        );
    }

    #[test]
    fn restore_if_the_prior_snapshot_fails_touches_nothing() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1", "one");
        write(&repo, "a.txt", b"2");
        let id = create(dir.path(), repo.path(), &paths(&["a.txt"]), "test").unwrap();
        write(&repo, "a.txt", "newer".as_bytes());
        let root = dir.path().join("snapshots");
        let mut permissions = fs::metadata(&root).unwrap().permissions();
        let original = permissions.clone();
        permissions.set_mode(0o500);
        fs::set_permissions(&root, permissions).unwrap();

        let result = restore(dir.path(), repo.path(), &id);

        fs::set_permissions(&root, original).unwrap();
        assert!(result.is_err());
        assert_eq!(read(&repo, "a.txt"), "newer".as_bytes());
    }

    #[test]
    fn restore_rejects_invalid_identifiers() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        for id in ["", "..", "../x", "a/b", ".hidden", "a b"] {
            let error = restore(dir.path(), repo.path(), id).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{id:?}");
        }
    }

    #[test]
    fn identifiers_sorted_and_unique() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        let same_instant = SystemTime::UNIX_EPOCH + Duration::from_millis(1_700_000_000_123);
        let later = same_instant + Duration::from_millis(1);

        let mut ids: Vec<_> = (0..50)
            .map(|_| create_at(dir.path(), repo.path(), &[], "test", same_instant).unwrap())
            .collect();
        ids.push(create_at(dir.path(), repo.path(), &[], "test", later).unwrap());

        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);
        assert_eq!(ids.iter().collect::<BTreeSet<_>>().len(), ids.len());
        let listed: Vec<_> = list(dir.path())
            .into_iter()
            .map(|snapshot| snapshot.id)
            .collect();
        assert_eq!(listed, sorted.into_iter().rev().collect::<Vec<_>>());
    }

    #[test]
    fn deletes_old_snapshots() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1", "one");
        let now = SystemTime::now();
        let day = Duration::from_secs(24 * 60 * 60);
        let old = create_at(
            dir.path(),
            repo.path(),
            &paths(&["a.txt"]),
            "old",
            now - day * 31,
        )
        .unwrap();
        let edge = create_at(
            dir.path(),
            repo.path(),
            &paths(&["a.txt"]),
            "limit",
            now - day * 29,
        )
        .unwrap();
        let recent = create(dir.path(), repo.path(), &paths(&["a.txt"]), "recent").unwrap();

        let removed = prune(dir.path(), now).unwrap();

        assert_eq!(removed, 1);
        assert!(!dir.path().join("snapshots").join(&old).exists());
        let ids: BTreeSet<_> = list(dir.path())
            .into_iter()
            .map(|snapshot| snapshot.id)
            .collect();
        assert_eq!(ids, BTreeSet::from([edge, recent]));
    }

    #[test]
    fn pruning_without_snapshots_folder_does_not_fail() {
        let dir = TempDir::new().unwrap();
        assert_eq!(prune(dir.path(), SystemTime::now()).unwrap(), 0);
        assert_eq!(
            prune(&dir.path().join("does-not-exist"), SystemTime::now()).unwrap(),
            0
        );
    }

    #[test]
    fn pruning_removes_old_remains_of_incomplete_snapshots() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("snapshots");
        let partial = root.join(".0000-0-1.partial");
        fs::create_dir_all(partial.join("files")).unwrap();
        let now = SystemTime::now();
        let old = now - Duration::from_secs(40 * 24 * 60 * 60);
        fs::File::open(&partial).unwrap().set_modified(old).unwrap();

        prune(dir.path(), now).unwrap();

        assert!(!partial.exists());
    }

    #[test]
    fn paths_differing_only_in_case_do_not_overwrite_each_other_in_the_snapshot() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        write(&repo, "Notes.txt", b"one");
        // On a volume that ignores case it is the same file; on another, two different ones.
        write(&repo, "notes.txt", b"two");

        let id = create(
            dir.path(),
            repo.path(),
            &paths(&["Notes.txt", "notes.txt"]),
            "test",
        )
        .unwrap();

        let folder = dir.path().join("snapshots").join(&id).join("files");
        assert_eq!(fs::read_dir(folder).unwrap().count(), 2);
        for path in ["Notes.txt", "notes.txt"] {
            let copy = stored(dir.path(), &id, path, false).unwrap();
            assert_eq!(fs::read(copy).unwrap(), read(&repo, path), "{path}");
        }
    }

    #[test]
    fn stage_blobs_of_paths_differing_only_in_case_do_not_overwrite_each_other() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("base.txt", b"1", "one");
        let scratch = TempDir::new().unwrap();
        for (name, contents) in [("Notes.txt", "one"), ("notes.txt", "two")] {
            let source = scratch.path().join("blob");
            fs::write(&source, contents).unwrap();
            let hash = repo.git(&["hash-object", "-w", source.to_str().unwrap()]);
            let entry = format!("100644,{},{name}", hash.trim());
            repo.git(&["update-index", "--add", "--cacheinfo", &entry]);
        }
        assert_eq!(repo.git(&["ls-files"]), "Notes.txt\nbase.txt\nnotes.txt\n");

        let id = create(
            dir.path(),
            repo.path(),
            &paths(&["Notes.txt", "notes.txt"]),
            "test",
        )
        .unwrap();

        for (path, contents) in [("Notes.txt", "one"), ("notes.txt", "two")] {
            let blob = stored(dir.path(), &id, path, true).unwrap();
            assert_eq!(fs::read(blob).unwrap(), contents.as_bytes(), "{path}");
        }
    }

    #[test]
    fn snapshot_fails_if_the_stage_content_cannot_be_read() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1", "one");
        write(&repo, "a.txt", b"2");
        repo.git(&["add", "a.txt"]);
        write(&repo, "a.txt", b"3");
        let hash = repo.git(&["rev-parse", ":a.txt"]);
        let hash = hash.trim();
        let object = repo
            .path()
            .join(".git/objects")
            .join(&hash[..2])
            .join(&hash[2..]);
        fs::remove_file(object).unwrap();

        let result = create(dir.path(), repo.path(), &paths(&["a.txt"]), "test");

        assert!(result.is_err());
        assert!(list(dir.path()).is_empty());
        let leftovers = fs::read_dir(dir.path().join("snapshots")).map_or(0, |d| d.count());
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn snapshot_of_conflicted_path_saves_no_blob_and_does_not_fail() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("f.txt", b"base\n", "base");
        repo.git(&["checkout", "-q", "-b", "other"]);
        repo.commit("f.txt", b"other\n", "other");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("f.txt", b"main\n", "main");
        let _ = crate::test_support::git_command()
            .current_dir(repo.path())
            .args(["merge", "--no-edit", "other"])
            .output()
            .unwrap();

        let id = create(dir.path(), repo.path(), &paths(&["f.txt"]), "test").unwrap();

        assert!(!stored(dir.path(), &id, "f.txt", true).unwrap().exists());
        assert!(stored(dir.path(), &id, "f.txt", false).unwrap().exists());
    }

    #[test]
    fn reading_the_stage_only_gives_absent_if_there_is_no_stage_zero() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        write(&repo, "a.txt", b"2");
        repo.git(&["add", "a.txt"]);

        assert_eq!(
            staged_blob(repo.path(), "a.txt").unwrap(),
            Some(("100644".to_string(), b"2".to_vec()))
        );
        assert_eq!(staged_blob(repo.path(), "not-there.txt").unwrap(), None);

        let hash = repo.git(&["rev-parse", ":a.txt"]);
        let hash = hash.trim();
        let object = repo
            .path()
            .join(".git/objects")
            .join(&hash[..2])
            .join(&hash[2..]);
        fs::remove_file(object).unwrap();

        // It is in the index but cannot be read: it is not "absent", it is a failure.
        assert!(staged_blob(repo.path(), "a.txt").is_err());
    }

    fn merge_conflict(repo: &TempRepo) {
        repo.git(&["checkout", "-q", "-b", "other"]);
        repo.commit("f.txt", b"other\n", "on other");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("f.txt", b"main\n", "on main");
        let merge = crate::test_support::git_command()
            .current_dir(repo.path())
            .args(["merge", "-q", "other"])
            .output()
            .unwrap();
        assert!(!merge.status.success());
    }

    #[test]
    fn restore_does_not_resolve_a_conflict_behind_the_scenes() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("f.txt", b"base\n", "base");
        write(&repo, "f.txt", b"in stage\n");
        repo.git(&["add", "f.txt"]);
        write(&repo, "f.txt", b"on disk\n");
        let id = create(dir.path(), repo.path(), &paths(&["f.txt"]), "test").unwrap();
        repo.git(&["checkout", "-q", "HEAD", "--", "f.txt"]);
        merge_conflict(&repo);
        let stages = repo.git(&["ls-files", "-s"]);
        let disk = fs::read(repo.path().join("f.txt")).unwrap();

        let error = restore(dir.path(), repo.path(), &id).unwrap_err();

        assert!(error.to_string().contains("f.txt"), "{error}");
        assert_eq!(repo.git(&["ls-files", "-s"]), stages);
        assert_eq!(fs::read(repo.path().join("f.txt")).unwrap(), disk);
        assert_eq!(list(dir.path()).len(), 1);
    }

    fn staged_and_changed(repo: &TempRepo, dir: &TempDir) -> String {
        repo.commit("a.txt", b"1\n", "one");
        repo.commit("b.txt", b"1\n", "two");
        write(repo, "a.txt", b"copy\n");
        write(repo, "b.txt", b"in stage\n");
        repo.git(&["add", "b.txt"]);
        write(repo, "b.txt", b"on disk\n");
        let id = create(dir.path(), repo.path(), &paths(&["a.txt", "b.txt"]), "test").unwrap();
        write(repo, "a.txt", b"new\n");
        id
    }

    #[test]
    fn restore_checks_the_stage_part_before_writing_anything() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        let id = staged_and_changed(&repo, &dir);
        fs::remove_file(stored(dir.path(), &id, "b.txt", true).unwrap()).unwrap();

        assert!(restore(dir.path(), repo.path(), &id).is_err());

        assert_eq!(fs::read(repo.path().join("a.txt")).unwrap(), b"new\n");
        assert_eq!(list(dir.path()).len(), 1);
    }

    #[test]
    fn restore_checks_the_stage_paths_before_writing_anything() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1\n", "one");
        write(&repo, "a.txt", b"copy\n");
        write(&repo, "new.txt", b"in stage\n");
        repo.git(&["add", "new.txt"]);
        fs::remove_file(repo.path().join("new.txt")).unwrap();
        let id = create(
            dir.path(),
            repo.path(),
            &paths(&["a.txt", "new.txt"]),
            "test",
        )
        .unwrap();
        write(&repo, "a.txt", b"new\n");
        let manifest = snapshots_root(dir.path()).join(&id).join(MANIFEST_FILE);
        let edited = fs::read_to_string(&manifest)
            .unwrap()
            .replace("new.txt", "../outside.txt");
        fs::write(&manifest, edited).unwrap();

        assert!(restore(dir.path(), repo.path(), &id).is_err());

        assert_eq!(fs::read(repo.path().join("a.txt")).unwrap(), b"new\n");
    }

    fn turned_directory(repo: &TempRepo) {
        repo.commit("a.txt", b"1\n", "base");
        write(repo, "a.txt", b"in stage\n");
        repo.git(&["add", "a.txt"]);
        fs::remove_file(repo.path().join("a.txt")).unwrap();
        write(repo, "a.txt/inside", b"2\n");
    }

    #[test]
    fn the_normal_snapshot_does_not_continue_without_being_able_to_copy_the_disk() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        turned_directory(&repo);

        assert!(create(dir.path(), repo.path(), &paths(&["a.txt"]), "test").is_err());
    }

    #[test]
    fn the_index_snapshot_saves_the_blob_even_if_there_is_a_folder() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        turned_directory(&repo);

        let id = create_for_index(dir.path(), repo.path(), &paths(&["a.txt"]), "test").unwrap();

        let entry = list(dir.path()).remove(0).entries.remove(0);
        assert_eq!(entry.kind, EntryKind::Absent);
        assert!(entry.staged_blob);
        assert_eq!(
            fs::read(stored(dir.path(), &id, "a.txt", true).unwrap()).unwrap(),
            b"in stage\n"
        );
    }

    #[test]
    fn the_index_snapshot_saves_the_blob_if_the_index_matches_head() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1\n", "base");
        fs::remove_file(repo.path().join("a.txt")).unwrap();
        write(&repo, "a.txt/inside", b"2\n");

        let id = create_for_index(dir.path(), repo.path(), &paths(&["a.txt"]), "test").unwrap();

        assert_eq!(
            fs::read(stored(dir.path(), &id, "a.txt", true).unwrap()).unwrap(),
            b"1\n"
        );
    }

    #[test]
    fn without_an_index_entry_a_folder_is_still_an_error() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1\n", "base");
        write(&repo, "folder/inside", b"2\n");

        let result = create_for_index(dir.path(), repo.path(), &paths(&["folder"]), "test");

        assert!(result.is_err());
    }

    #[test]
    fn the_index_snapshot_saves_what_belongs_to_a_folder_that_is_now_a_file() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("d/f", b"1\n", "base");
        write(&repo, "d/f", b"in stage\n");
        repo.git(&["add", "d/f"]);
        fs::remove_dir_all(repo.path().join("d")).unwrap();
        write(&repo, "d", b"file\n");

        let id = create_for_index(dir.path(), repo.path(), &paths(&["d/f"]), "test").unwrap();

        let entry = list(dir.path()).remove(0).entries.remove(0);
        assert_eq!(entry.kind, EntryKind::Absent);
        assert_eq!(
            fs::read(stored(dir.path(), &id, "d/f", true).unwrap()).unwrap(),
            b"in stage\n"
        );
    }

    #[test]
    fn restore_does_not_continue_if_a_folder_occupies_the_place_of_a_file() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1\n", "base");
        write(&repo, "a.txt", b"copy\n");
        let id = create(dir.path(), repo.path(), &paths(&["a.txt"]), "test").unwrap();
        fs::remove_file(repo.path().join("a.txt")).unwrap();
        write(&repo, "a.txt/inside", b"2\n");

        let error = restore(dir.path(), repo.path(), &id).unwrap_err();

        assert!(error.to_string().contains("a.txt"), "{error}");
        assert_eq!(list(dir.path()).len(), 1);
        assert!(repo.path().join("a.txt/inside").exists());
    }

    #[test]
    fn restore_does_not_continue_if_a_file_occupies_the_place_of_a_folder() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("d/f", b"1\n", "base");
        write(&repo, "d/f", b"copy\n");
        let id = create(dir.path(), repo.path(), &paths(&["d/f"]), "test").unwrap();
        fs::remove_dir_all(repo.path().join("d")).unwrap();
        write(&repo, "d", b"file\n");

        let error = restore(dir.path(), repo.path(), &id).unwrap_err();

        assert!(error.to_string().contains("d/f"), "{error}");
        assert_eq!(list(dir.path()).len(), 1);
        assert_eq!(fs::read(repo.path().join("d")).unwrap(), b"file\n");
    }

    fn assert_refused(repo: &TempRepo, dir: &TempDir, id: &str, clash: &str) {
        let index = repo.git(&["ls-files", "-s"]);
        let copies = list(dir.path()).len();

        let error = restore(dir.path(), repo.path(), id)
            .unwrap_err()
            .to_string();

        assert!(error.contains(clash), "{error}");
        assert!(error.contains("snapshot"), "{error}");
        assert_eq!(repo.git(&["ls-files", "-s"]), index);
        assert_eq!(list(dir.path()).len(), copies);
    }

    #[test]
    fn restore_does_not_remove_from_the_stage_what_is_below_the_path() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1\n", "base");
        write(&repo, "a.txt", b"in stage\n");
        repo.git(&["add", "a.txt"]);
        fs::remove_file(repo.path().join("a.txt")).unwrap();
        let id = create(dir.path(), repo.path(), &paths(&["a.txt"]), "test").unwrap();
        write(&repo, "a.txt/inside", b"staged\n");
        repo.git(&["rm", "-q", "--cached", "a.txt"]);
        repo.git(&["add", "a.txt/inside"]);

        assert_refused(&repo, &dir, &id, "a.txt/inside");
    }

    // `d/f` copied only from the index, and in its place `d` staged as a file.
    fn ancestor_in_stage(repo: &TempRepo, dir: &TempDir) -> String {
        repo.commit("d/f", b"1\n", "base");
        write(repo, "d/f", b"in stage\n");
        repo.git(&["add", "d/f"]);
        fs::remove_dir_all(repo.path().join("d")).unwrap();
        write(repo, "d", b"staged\n");
        let id = create_for_index(dir.path(), repo.path(), &paths(&["d/f"]), "test").unwrap();
        repo.git(&["rm", "-q", "-f", "--cached", "d/f"]);
        repo.git(&["add", "d"]);
        id
    }

    #[test]
    fn restore_does_not_remove_from_the_stage_a_file_in_the_way_if_it_is_on_disk() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        let id = ancestor_in_stage(&repo, &dir);

        assert_refused(&repo, &dir, &id, "\"d\"");
    }

    #[test]
    fn restore_does_not_remove_from_the_stage_a_file_in_the_way_with_nothing_on_disk() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        let id = ancestor_in_stage(&repo, &dir);
        fs::remove_file(repo.path().join("d")).unwrap();

        assert_refused(&repo, &dir, &id, "\"d\"");
    }

    #[test]
    fn restore_the_stage_part_without_an_entry_in_the_way() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1\n", "base");
        write(&repo, "new.txt", b"in stage\n");
        repo.git(&["add", "new.txt"]);
        fs::remove_file(repo.path().join("new.txt")).unwrap();
        write(&repo, "new.txt/inside", b"2\n");
        let id = create_for_index(dir.path(), repo.path(), &paths(&["new.txt"]), "test").unwrap();
        repo.git(&["rm", "-q", "--cached", "new.txt"]);

        restore(dir.path(), repo.path(), &id).unwrap();

        assert_eq!(repo.git(&["show", ":new.txt"]), "in stage\n");
        assert!(repo.path().join("new.txt/inside").exists());
    }

    #[test]
    fn the_entries_a_path_would_displace() {
        let repo = TempRepo::new();
        repo.commit("a/b", b"1\n", "one");
        repo.commit("a/c/d", b"1\n", "two");
        repo.commit("z", b"1\n", "three");

        let below = index_clashes(repo.path(), &paths(&["a"])).unwrap();
        let above = index_clashes(repo.path(), &paths(&["z/inside/more"])).unwrap();
        let free = index_clashes(repo.path(), &paths(&["a/b", "a/new", "ab"])).unwrap();

        assert_eq!(
            below,
            BTreeSet::from(["a/b".to_string(), "a/c/d".to_string()])
        );
        assert_eq!(above, BTreeSet::from(["z".to_string()]));
        assert_eq!(free, BTreeSet::new());
    }

    #[test]
    fn a_link_in_the_way_displaces_the_path() {
        let repo = TempRepo::new();
        fs::create_dir(repo.path().join("e")).unwrap();
        write(&repo, "e/f", b"1\n");
        symlink("e", repo.path().join("d")).unwrap();
        symlink("e/f", repo.path().join("link")).unwrap();

        assert!(displaced(repo.path(), "d/f"));
        assert!(!displaced(repo.path(), "link"));
        assert!(!displaced(repo.path(), "e/f"));
        assert!(!displaced(repo.path(), "missing/path"));
    }
}
