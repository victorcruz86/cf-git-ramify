// `Get` (`GetVersionAction`, `CompareWithLocalDialog.GetVersionActionProvider`): the files as a
// revision had them, written over the local ones. Before any write it takes one recoverable copy of
// every local file it will touch; a file that changes after that copy is left as it is and reported.
// It writes only the files: no `git add` and no `git rm` (decision 6).

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::commands::{
    blocking_api, current_repo, flush_diff_window, repo_data_dir, ApiError, AppState, RepoLocks,
    SAVE_ACTION,
};
use crate::commit;
use crate::diff;
use crate::git::{self, GitError};
use crate::operation;
use crate::snapshots;
use crate::storage;
use crate::worktree_diff::content_hash;

pub const BEFORE_GET: &str = "before Get";

const TREE_MODE: &str = "040000";
const GITLINK_MODE: &str = "160000";
const SYMLINK_MODE: &str = "120000";
const EXECUTABLE_MODE: &str = "100755";
// What a file that did not exist gets, before the executable bits of its revision.
const NEW_FILE_MODE: u32 = 0o644;
const OWNER_WRITE: u32 = 0o200;
const FOLDER_STAMP: &str = "folder";

// A file to get: `path` is where it is now (or would be, if it is gone), `old_path` the path it had
// in `revision` when that is another one. The revision is read at `old_path`, else at `path`, and
// the file is written there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetItem {
    pub path: String,
    pub revision: String,
    #[serde(default)]
    pub old_path: Option<String>,
}

impl GetItem {
    fn target(&self) -> &str {
        self.old_path.as_deref().unwrap_or(&self.path)
    }
}

// What the `Get Revision` confirmation needs (`ReplaceFileConfirmationDialog`): the local files
// that exist and are not `NOT_CHANGED`, and the HEAD the write must still find.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetCheck {
    pub root: String,
    pub head: String,
    pub modified: Vec<String>,
    pub seen: Vec<Seen>,
}

// What the check read of a path the Get will touch: the hash of its content, `None` if it was not
// there. The write leaves a file that no longer has it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Seen {
    pub path: String,
    pub hash: Option<String>,
}

// The moments of a Get at which another program may write (Devin, in the tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Moment {
    Copied,
    BeforeReplace(String),
    BeforeRemove(String),
    // Right after the last comparison of a path's content, before it is replaced or removed.
    Compared(String),
    // A link's place was emptied and the link is not made yet.
    Removed(String),
    // Right after a path was replaced.
    Replaced(String),
}

// The copy taken first, and the files left as they were because they changed after it. An item
// written at the old path of a rename whose local file was left is in both lists (`Done`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Got {
    pub copy: String,
    pub written: Vec<String>,
    pub changed: Vec<String>,
}

// `VcsException` (the revision could not be read) and `IOException` (it could not be written), which
// 241 shows as `Cannot load revision: {0}` and `Cannot save content: {0}`.
#[derive(Debug)]
pub enum GetError {
    Load(String),
    Save(String),
    // A write failed after the copy: what was written and left is said, with the copy to restore.
    Interrupted {
        message: String,
        copy: String,
        written: Vec<String>,
        changed: Vec<String>,
    },
    Api(ApiError),
}

impl From<GetError> for ApiError {
    fn from(error: GetError) -> Self {
        match error {
            GetError::Load(message) => ApiError {
                kind: "load_failed",
                message,
                details: None,
            },
            GetError::Save(message) => ApiError {
                kind: "save_failed",
                message,
                details: None,
            },
            GetError::Interrupted {
                message,
                copy,
                written,
                changed,
            } => ApiError {
                kind: "save_failed",
                message,
                details: Some(serde_json::json!({
                    "copy": copy,
                    "written": written,
                    "changed": changed,
                })),
            },
            GetError::Api(error) => error,
        }
    }
}

fn load(error: GitError) -> GetError {
    GetError::Load(ApiError::from(error).message)
}

fn save(error: io::Error) -> GetError {
    GetError::Save(error.to_string())
}

// What the revision has at a path.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Content {
    Absent,
    File { bytes: Vec<u8>, executable: bool },
    Link(Vec<u8>),
}

// The entry of `path` in `revision`, as `git ls-tree` lists it: mode and object.
fn entry_of(repo: &Path, revision: &str, path: &str) -> Result<Option<(String, String)>, GitError> {
    let args = git::exact(&["--literal-pathspecs", "ls-tree", "-z", revision, "--", path]);
    let listed = git::run(repo, &args)?;
    for line in listed.split(|byte| *byte == 0) {
        let Some(tab) = line.iter().position(|byte| *byte == b'\t') else {
            continue;
        };
        if &line[tab + 1..] != path.as_bytes() {
            continue;
        }
        let head = String::from_utf8_lossy(&line[..tab]);
        if let [mode, _, object] = head.split_whitespace().collect::<Vec<_>>()[..] {
            return Ok(Some((mode.to_string(), object.to_string())));
        }
    }
    Ok(None)
}

// The bytes a checkout would write (`git.read.content.with` is `FILTERS` in 241): the blob through
// the working-tree filters of its path.
fn read_content(repo: &Path, revision: &str, path: &str) -> Result<Content, GetError> {
    diff::check_hash(revision).map_err(load)?;
    let Some((mode, object)) = entry_of(repo, revision, path).map_err(load)? else {
        return Ok(Content::Absent);
    };
    let short = &revision[..revision.len().min(8)];
    match mode.as_str() {
        TREE_MODE => Err(GetError::Load(format!("\"{path}\" is a folder in {short}"))),
        GITLINK_MODE => Err(GetError::Load(format!(
            "\"{path}\" is a submodule in {short}"
        ))),
        SYMLINK_MODE => {
            let target = git::run(repo, &["cat-file", "blob", &object]).map_err(load)?;
            Ok(Content::Link(target))
        }
        _ => {
            let at = format!("--path={path}");
            let args = git::exact(&["cat-file", "--filters", &at, &object]);
            let bytes = git::run(repo, &args).map_err(load)?;
            Ok(Content::File {
                bytes,
                executable: mode == EXECUTABLE_MODE,
            })
        }
    }
}

fn exists(path: &Path) -> bool {
    path.symlink_metadata().is_ok()
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

// What a path holds, as the check and the copy are compared: the hash of a file's bytes or of a
// link's target, `None` if nothing is there.
fn stamp_of(path: &Path) -> io::Result<Option<String>> {
    match fs::symlink_metadata(path) {
        Ok(meta) => stamp_with(path, &meta),
        Err(error) if gone(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

fn gone(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

// The stamp of what `meta`, the `lstat` of `path`, describes. A path removed after that `lstat`
// holds nothing, as if the `lstat` had not found it: the Get leaves it, and goes on.
fn stamp_with(path: &Path, meta: &fs::Metadata) -> io::Result<Option<String>> {
    let read = if meta.is_symlink() {
        fs::read_link(path)
            .map(|link| format!("link:{}", content_hash(link.as_os_str().as_bytes())))
    } else if meta.is_dir() {
        Ok(FOLDER_STAMP.to_string())
    } else {
        fs::read(path).map(|bytes| content_hash(&bytes))
    };
    match read {
        Ok(stamp) => Ok(Some(stamp)),
        Err(error) if gone(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

// The stamp `stamp_of` gives a path once `content` is written there.
fn written_stamp(content: &Content) -> Option<String> {
    match content {
        Content::Absent => None,
        Content::File { bytes, .. } => Some(content_hash(bytes)),
        Content::Link(link) => Some(format!("link:{}", content_hash(link))),
    }
}

// What `lstat` says of a path, `None` if nothing is there: a write changes the size or the times,
// a replacement the inode.
type Seal = Option<(u64, u64, u64, i64, i64, i64, i64, u32)>;

fn seal_of(path: &Path) -> io::Result<Seal> {
    match fs::symlink_metadata(path) {
        Ok(meta) => Ok(Some((
            meta.dev(),
            meta.ino(),
            meta.size(),
            meta.mtime(),
            meta.mtime_nsec(),
            meta.ctime(),
            meta.ctime_nsec(),
            meta.mode(),
        ))),
        Err(error) if gone(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

// Per path, what it must still hold for the Get to touch it: what the check read, when the copy
// holds the same. A path missing here is not touched.
type Expected = BTreeMap<String, Option<String>>;

fn still(repo: &Path, expected: &Expected, path: &str) -> io::Result<bool> {
    still_with(repo, expected, path, || {})
}

// The content is read whole, and a write while it is read, or right after, would go unseen: the
// `lstat` before the read and the one after it must agree too. Only a write between that last
// `lstat` and the rename or the removal is not seen; POSIX has no way to close that instant.
// `compared` runs right after the content was compared.
fn still_with(
    repo: &Path,
    expected: &Expected,
    path: &str,
    compared: impl FnOnce(),
) -> io::Result<bool> {
    let Some(stamp) = expected.get(path) else {
        return Ok(false);
    };
    let target = repo.join(path);
    let before = seal_of(&target)?;
    let same = *stamp == stamp_of(&target)?;
    compared();
    Ok(same && seal_of(&target)? == before)
}

fn expected_of(dir: &Path, copy: &str, paths: &[String], seen: &[Seen]) -> io::Result<Expected> {
    let stored = snapshots::stored_files(dir, copy)?;
    let mut expected = Expected::new();
    for path in paths {
        let Some(read) = seen.iter().find(|seen| &seen.path == path) else {
            continue;
        };
        let copied = match stored.get(path) {
            Some(slot) => stamp_of(slot)?,
            None => continue,
        };
        if copied == read.hash {
            expected.insert(path.clone(), copied);
        }
    }
    Ok(expected)
}

// An existing file keeps its permissions, made writable (`ensureFilesWritable`), with the
// executable bits of the revision.
fn mode_for(target: &Path, executable: bool) -> u32 {
    let base = target
        .symlink_metadata()
        .ok()
        .filter(fs::Metadata::is_file)
        .map_or(NEW_FILE_MODE, |meta| {
            (meta.permissions().mode() & 0o7777) | OWNER_WRITE
        });
    if executable {
        base | ((base & 0o444) >> 2)
    } else {
        base & !0o111
    }
}

// Two names of one file, as on a volume that does not tell case apart.
fn same_file(a: &Path, b: &Path) -> bool {
    match (a.symlink_metadata(), b.symlink_metadata()) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

// Whether the item was written, or left because a path changed right before it was replaced.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Written,
    Left,
}

// Each path is compared with what it must hold right before it is replaced or removed: what
// someone writes after that, in the instant before the rename, is still not covered.
struct Writer<'a, P: FnMut(&Moment)> {
    repo: &'a Path,
    expected: Expected,
    pause: P,
    // The path a write emptied before it failed: what it held is only in the copy.
    removed: Option<String>,
}

impl<P: FnMut(&Moment)> Writer<'_, P> {
    fn remove(&mut self, path: &str) -> io::Result<Outcome> {
        (self.pause)(&Moment::BeforeRemove(path.to_string()));
        let pause = &mut self.pause;
        let compared = || pause(&Moment::Compared(path.to_string()));
        if !still_with(self.repo, &self.expected, path, compared)? {
            return Ok(Outcome::Left);
        }
        remove_if_present(&self.repo.join(path))?;
        self.expected.insert(path.to_string(), None);
        Ok(Outcome::Written)
    }

    // Through `write_atomic_if`: a link at the place is replaced, never followed.
    fn write(&mut self, path: &str, content: &Content) -> io::Result<Outcome> {
        let target = self.repo.join(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let (repo, expected, pause) = (self.repo, &self.expected, &mut self.pause);
        let mut ready = || {
            pause(&Moment::BeforeReplace(path.to_string()));
            still_with(repo, expected, path, || {
                pause(&Moment::Compared(path.to_string()))
            })
        };
        let replaced = match content {
            Content::Absent => return self.remove(path),
            Content::File { bytes, executable } => {
                let permissions = fs::Permissions::from_mode(mode_for(&target, *executable));
                storage::write_atomic_if(&target, bytes, Some(permissions), ready)?
            }
            Content::Link(link) => {
                if ready()? {
                    remove_if_present(&target)?;
                    self.removed = Some(path.to_string());
                    pause(&Moment::Removed(path.to_string()));
                    std::os::unix::fs::symlink(OsStr::from_bytes(link), &target)?;
                    self.removed = None;
                    true
                } else {
                    false
                }
            }
        };
        if !replaced {
            return Ok(Outcome::Left);
        }
        (self.pause)(&Moment::Replaced(path.to_string()));
        // What was just written, not what the disk holds a moment later: a write that lands in
        // that instant is then seen as a change by a later item of the same Get, and left.
        self.expected
            .insert(path.to_string(), written_stamp(content));
        Ok(Outcome::Written)
    }

    // `MyWriteVersionTask.run`, per file: no content deletes it; otherwise it is written at the path
    // it had in the revision, and the local file at another path goes away (the rename or move
    // back).
    fn apply(&mut self, item: &GetItem, content: &Content, done: &mut Done) -> io::Result<()> {
        if *content == Content::Absent {
            let outcome = self.remove(&item.path)?;
            done.add(outcome, &item.path);
            return Ok(());
        }
        let outcome = match self.write(item.target(), content) {
            Ok(outcome) => outcome,
            Err(error) => {
                // A link whose place was emptied: reported written, so that Restore is offered.
                if let Some(path) = self.removed.take() {
                    done.written.push(path);
                }
                return Err(error);
            }
        };
        if outcome == Outcome::Left {
            done.changed.push(item.path.clone());
            return Ok(());
        }
        let (local, target) = (self.repo.join(&item.path), self.repo.join(item.target()));
        if item.target() == item.path || !exists(&local) || same_file(&local, &target) {
            done.written.push(item.path.clone());
            return Ok(());
        }
        // The revision's file is already written at the old path. If the local file at the new
        // one is left, or cannot be removed, the old path is still reported written: what it held
        // is in the copy, and the page offers Restore only for what was written.
        match self.remove(&item.path) {
            Ok(Outcome::Written) => done.written.push(item.path.clone()),
            Ok(Outcome::Left) => {
                done.written.push(item.target().to_string());
                done.changed.push(item.path.clone());
            }
            Err(error) => {
                done.written.push(item.target().to_string());
                return Err(error);
            }
        }
        Ok(())
    }
}

// What the Get did so far: per item its path, written or left; an item written only at the old
// path of a rename, its local file left, is in both, with the old path in `written`.
#[derive(Default)]
struct Done {
    written: Vec<String>,
    changed: Vec<String>,
}

impl Done {
    fn add(&mut self, outcome: Outcome, path: &str) {
        match outcome {
            Outcome::Written => self.written.push(path.to_string()),
            Outcome::Left => self.changed.push(path.to_string()),
        }
    }
}

fn touched(item: &GetItem) -> Vec<String> {
    let mut paths = vec![item.path.clone()];
    if item.target() != item.path {
        paths.push(item.target().to_string());
    }
    paths
}

fn touched_paths(items: &[GetItem]) -> Vec<String> {
    let paths: BTreeSet<String> = items.iter().flat_map(touched).collect();
    paths.into_iter().collect()
}

// The order of `rollback`: HEAD, the paths, the revisions read, the copy, and only then the disk.
// A path is touched only if it still holds what the check read (`seen`), as the copy does too.
// `pause` lets the tests write at each moment another program could.
fn get_pausing(
    repo: &Path,
    dir: &Path,
    items: &[GetItem],
    expected_head: &str,
    seen: &[Seen],
    mut pause: impl FnMut(&Moment),
) -> Result<Got, GetError> {
    operation::check_head(repo, expected_head).map_err(|error| GetError::Api(error.into()))?;
    let paths = touched_paths(items);
    for path in &paths {
        snapshots::check_path(repo, path).map_err(save)?;
    }
    let contents = items
        .iter()
        .map(|item| read_content(repo, &item.revision, item.target()))
        .collect::<Result<Vec<_>, _>>()?;
    let copy = snapshots::create(dir, repo, &paths, BEFORE_GET).map_err(|error| {
        GetError::Save(format!(
            "Could not save the backup copy and nothing was touched: {error}"
        ))
    })?;
    pause(&Moment::Copied);
    let mut done = Done::default();
    let interrupted = |error: io::Error, done: Done| GetError::Interrupted {
        message: error.to_string(),
        copy: copy.clone(),
        written: done.written,
        changed: done.changed,
    };
    let expected = match expected_of(dir, &copy, &paths, seen) {
        Ok(expected) => expected,
        Err(error) => return Err(interrupted(error, done)),
    };
    let mut writer = Writer {
        repo,
        expected,
        pause,
        removed: None,
    };
    for (item, content) in items.iter().zip(&contents) {
        let applied = touched(item)
            .iter()
            .try_fold(true, |all, path| {
                Ok(all && still(repo, &writer.expected, path)?)
            })
            .and_then(|unchanged| {
                if unchanged {
                    writer.apply(item, content, &mut done)
                } else {
                    done.changed.push(item.path.clone());
                    Ok(())
                }
            });
        if let Err(error) = applied {
            return Err(interrupted(error, done));
        }
    }
    Ok(Got {
        copy,
        written: done.written,
        changed: done.changed,
    })
}

pub fn get(
    repo: &Path,
    dir: &Path,
    items: &[GetItem],
    expected_head: &str,
    seen: &[Seen],
) -> Result<Got, GetError> {
    get_pausing(repo, dir, items, expected_head, seen, |_| {})
}

// `git status` of those paths, ignored and untracked files included: any entry is a status other
// than `NOT_CHANGED`.
fn with_status(repo: &Path, paths: &[String]) -> Result<BTreeSet<String>, GitError> {
    let head = git::exact(&[
        "--literal-pathspecs",
        "status",
        "--porcelain",
        "-z",
        "--ignored",
        "--untracked-files=all",
        "--",
    ]);
    let mut found = BTreeSet::new();
    for chunk in paths.chunks(git::PATHS_PER_COMMAND) {
        let mut args = head.clone();
        args.extend(chunk.iter().map(String::as_str));
        let output = git::run(repo, &args)?;
        let mut fields = output.split(|byte| *byte == 0);
        while let Some(field) = fields.next() {
            if field.len() < 4 {
                continue;
            }
            found.insert(String::from_utf8_lossy(&field[3..]).into_owned());
            // A rename or a copy is followed by its original path.
            if matches!(field[0], b'R' | b'C') || matches!(field[1], b'R' | b'C') {
                fields.next();
            }
        }
    }
    Ok(found)
}

// `ReplaceFileConfirmationDialog.collectModifiedFiles` over the local files that exist, the old
// path of a rename included (the Get writes there too), and what each touched path holds now.
pub fn check(repo: &Path, items: &[GetItem]) -> Result<GetCheck, GitError> {
    let paths: Vec<String> = touched_paths(items)
        .into_iter()
        .filter(|path| snapshots::check_path(repo, path).is_ok())
        .collect();
    let mut seen = Vec::new();
    for path in &paths {
        let hash = stamp_of(&repo.join(path)).map_err(|error| GitError::Io(error.to_string()))?;
        seen.push(Seen {
            path: path.clone(),
            hash,
        });
    }
    let present: Vec<String> = seen
        .iter()
        .filter(|seen| seen.hash.is_some())
        .map(|seen| seen.path.clone())
        .collect();
    let changed = with_status(repo, &present)?;
    Ok(GetCheck {
        root: repo.to_string_lossy().into_owned(),
        head: operation::head_of(repo)?,
        modified: present
            .into_iter()
            .filter(|path| changed.contains(path))
            .collect(),
        seen,
    })
}

pub fn get_locked(
    locks: &RepoLocks,
    repo: &Path,
    dir: &Path,
    items: &[GetItem],
    expected_head: &str,
    seen: &[Seen],
) -> Result<Got, ApiError> {
    locks.run(repo, || {
        commit::ensure_idle(repo, dir)?;
        Ok(get(repo, dir, items, expected_head, seen)?)
    })
}

// The diff window is saved first: an edit of its local side is a local modification to ask about.
#[tauri::command]
pub async fn revision_get_check(
    items: Vec<GetItem>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<GetCheck, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let repo = current_repo(&state)?;
    blocking_api(move || Ok(check(&repo, &items)?)).await
}

// Before touching anything it saves a recoverable copy; returns its identifier.
#[tauri::command]
pub async fn revision_get(
    items: Vec<GetItem>,
    expected_head: String,
    seen: Vec<Seen>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Got, ApiError> {
    flush_diff_window(&app, &state, SAVE_ACTION).await?;
    let repo = current_repo(&state)?;
    let dir = repo_data_dir(&app, &state)?;
    let locks = state.operations.clone();
    blocking_api(move || get_locked(&locks, &repo, &dir, &items, &expected_head, &seen)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempRepo;
    use tempfile::TempDir;

    const NFD_NAME: &str = "cafe\u{301}.txt";

    fn item(path: &str, revision: &str, old_path: Option<&str>) -> GetItem {
        GetItem {
            path: path.to_string(),
            revision: revision.to_string(),
            old_path: old_path.map(str::to_string),
        }
    }

    fn read(repo: &TempRepo, path: &str) -> String {
        fs::read_to_string(repo.path().join(path)).unwrap()
    }

    fn head(repo: &TempRepo) -> String {
        repo.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    // The page's order: the check, then the Get with what it read.
    fn get_checked(repo: &TempRepo, dir: &Path, items: &[GetItem]) -> Result<Got, GetError> {
        let checked = check(repo.path(), items).unwrap();
        get(repo.path(), dir, items, &checked.head, &checked.seen)
    }

    fn pausing(
        repo: &TempRepo,
        dir: &Path,
        items: &[GetItem],
        pause: impl FnMut(&Moment),
    ) -> Result<Got, GetError> {
        let checked = check(repo.path(), items).unwrap();
        get_pausing(repo.path(), dir, items, &checked.head, &checked.seen, pause)
    }

    fn status(repo: &TempRepo) -> String {
        repo.git(&["status", "--porcelain", "--untracked-files=all"])
    }

    // What the copy kept of a path, `None` if it noted the path as absent.
    fn copied(dir: &Path, copy: &str, path: &str) -> Option<String> {
        snapshots::stored(dir, copy, path, false)
            .filter(|file| file.exists())
            .map(|file| fs::read_to_string(file).unwrap())
    }

    #[test]
    fn get_over_a_modified_file_asks_overwrite_modified_file_and_cancel_stops() {
        let repo = TempRepo::new();
        let first = repo.commit("a.txt", b"one\n", "first");
        repo.commit("b.txt", b"b\n", "second");
        fs::write(repo.path().join(".gitignore"), "*.log\n").unwrap();
        fs::write(repo.path().join("a.txt"), "edited\n").unwrap();
        fs::write(repo.path().join("new.txt"), "new\n").unwrap();
        fs::write(repo.path().join("x.log"), "log\n").unwrap();
        fs::write(repo.path().join("recreated.txt"), "untracked\n").unwrap();
        let items = [
            item("renamed.txt", &first, Some("recreated.txt")),
            item("a.txt", &first, None),
            item("b.txt", &first, None),
            item("new.txt", &first, None),
            item("x.log", &first, None),
            item("gone.txt", &first, None),
        ];

        let checked = check(repo.path(), &items).unwrap();

        // Unchanged and missing files are not asked about; untracked and ignored ones are, and so
        // is a file at the old path of a rename, which the Get would overwrite.
        assert_eq!(
            checked.modified,
            ["a.txt", "new.txt", "recreated.txt", "x.log"]
        );
        let seen = |path: &str| checked.seen.iter().find(|seen| seen.path == path).cloned();
        assert_eq!(seen("gone.txt").unwrap().hash, None);
        assert_eq!(
            seen("a.txt").unwrap().hash,
            Some(crate::worktree_diff::content_hash(b"edited\n"))
        );
        assert!(seen("renamed.txt").is_some() && seen("recreated.txt").is_some());
        assert_eq!(checked.head, head(&repo));
        assert_eq!(checked.root, repo.path().to_string_lossy());
        // The question and Cancel are the page's (`get.test.ts`): asking writes nothing.
        assert_eq!(read(&repo, "a.txt"), "edited\n");
    }

    #[test]
    fn get_takes_the_copy_first_and_leaves_a_file_devin_changed_meanwhile() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"a1\n", "first");
        let first = repo.commit("b.txt", b"b1\n", "second");
        repo.commit("a.txt", b"a2\n", "third");
        repo.commit("b.txt", b"b2\n", "fourth");
        fs::write(repo.path().join("a.txt"), "a local\n").unwrap();
        let items = [item("a.txt", &first, None), item("b.txt", &first, None)];

        let got = pausing(&repo, dir.path(), &items, |moment| {
            if *moment == Moment::Copied {
                fs::write(repo.path().join("b.txt"), "written by Devin meanwhile\n").unwrap();
            }
        })
        .unwrap();

        assert_eq!(got.changed, ["b.txt"]);
        assert_eq!(read(&repo, "a.txt"), "a1\n");
        assert_eq!(read(&repo, "b.txt"), "written by Devin meanwhile\n");
        // The copy has what each file was before the Get started.
        assert_eq!(
            copied(dir.path(), &got.copy, "a.txt").as_deref(),
            Some("a local\n")
        );
        assert_eq!(
            copied(dir.path(), &got.copy, "b.txt").as_deref(),
            Some("b2\n")
        );
        let snapshot = &snapshots::list(dir.path())[0];
        assert_eq!(snapshot.reason, BEFORE_GET);
    }

    #[test]
    fn get_of_a_deleted_revision_deletes_and_of_a_renamed_one_writes_the_old_path() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        let first = repo.commit("dir/old.txt", b"old content\n", "first");
        fs::create_dir(repo.path().join("moved")).unwrap();
        repo.git(&["mv", "dir/old.txt", "moved/new.txt"]);
        fs::write(repo.path().join("moved/new.txt"), "new content\n").unwrap();
        repo.commit("added.txt", b"added later\n", "rename and add");
        let items = [
            item("added.txt", &first, None),
            item("moved/new.txt", &first, Some("dir/old.txt")),
        ];

        let got = get_checked(&repo, dir.path(), &items).unwrap();

        assert!(got.changed.is_empty());
        assert!(!repo.path().join("added.txt").exists());
        assert!(!repo.path().join("moved/new.txt").exists());
        assert_eq!(read(&repo, "dir/old.txt"), "old content\n");
        // The copy can bring all of it back.
        assert_eq!(
            copied(dir.path(), &got.copy, "added.txt").as_deref(),
            Some("added later\n")
        );
        assert_eq!(
            copied(dir.path(), &got.copy, "moved/new.txt").as_deref(),
            Some("new content\n")
        );
        assert_eq!(copied(dir.path(), &got.copy, "dir/old.txt"), None);
    }

    #[test]
    fn get_restores_the_executable_bit_and_a_symlink_of_the_revision() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        fs::write(repo.path().join("run.sh"), "#!/bin/sh\n").unwrap();
        fs::set_permissions(
            repo.path().join("run.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        std::os::unix::fs::symlink("run.sh", repo.path().join("link")).unwrap();
        let first = repo.commit("x.txt", b"x\n", "first");
        fs::set_permissions(
            repo.path().join("run.sh"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        fs::remove_file(repo.path().join("link")).unwrap();
        fs::write(repo.path().join("link"), "a file now\n").unwrap();
        repo.commit("x.txt", b"x2\n", "second");

        let items = [item("run.sh", &first, None), item("link", &first, None)];
        get_checked(&repo, dir.path(), &items).unwrap();

        let mode = fs::metadata(repo.path().join("run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111);
        assert_eq!(
            fs::read_link(repo.path().join("link")).unwrap(),
            Path::new("run.sh")
        );
    }

    #[test]
    fn get_refuses_before_writing_when_head_moved() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        let first = repo.commit("a.txt", b"one\n", "first");
        let seen = repo.commit("a.txt", b"two\n", "second");
        // A commit from the terminal after the list was read.
        repo.commit("other.txt", b"other\n", "from the terminal");

        let items = [item("a.txt", &first, None)];
        let checked = check(repo.path(), &items).unwrap();
        let refused = get(repo.path(), dir.path(), &items, &seen, &checked.seen);

        let Err(GetError::Api(error)) = refused else {
            panic!("expected a refusal: {refused:?}");
        };
        assert_eq!(error.message, "Repository has already been changed");
        assert_eq!(read(&repo, "a.txt"), "two\n");
        assert!(snapshots::list(dir.path()).is_empty());
    }

    #[test]
    fn get_runs_no_git_add_or_rm() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        let first = repo.commit("kept.txt", b"kept\n", "first");
        repo.git(&["rm", "-q", "kept.txt"]);
        repo.commit("later.txt", b"later\n", "delete kept, add later");
        let index_before = repo.git(&["ls-files", "-s"]);

        let items = [
            item("kept.txt", &first, None),
            item("later.txt", &first, None),
        ];
        get_checked(&repo, dir.path(), &items).unwrap();

        // The file got back is unversioned, the one deleted is only gone from the disk.
        assert_eq!(repo.git(&["ls-files", "-s"]), index_before);
        assert_eq!(status(&repo), " D later.txt\n?? kept.txt\n");
        let source = include_str!("get_version.rs");
        let source = &source[..source.find("#[cfg(test)]").unwrap()];
        for command in ["\"add\"", "\"rm\"", "\"update-index\"", "\"checkout\""] {
            assert!(!source.contains(command), "{command}");
        }
    }

    // An entry only in the index, with the exact bytes of its name.
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
    fn get_reaches_a_glob_named_a_dash_named_and_an_nfd_file() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.git(&["config", "core.precomposeunicode", "true"]);
        let names = ["[ab]*.txt", "a.txt", "-x.txt", ":(glob)b.txt"];
        for name in names {
            fs::write(repo.path().join(name), format!("{name} one\n")).unwrap();
        }
        repo.git(&["add", "-A"]);
        add_exact(&repo, NFD_NAME, b"nfd one\n");
        repo.git(&["commit", "-q", "-m", "first"]);
        let first = head(&repo);
        for name in names {
            fs::write(repo.path().join(name), format!("{name} two\n")).unwrap();
        }
        repo.commit("z.txt", b"z\n", "second");

        let items = [
            item("[ab]*.txt", &first, None),
            item("-x.txt", &first, None),
            item(":(glob)b.txt", &first, None),
            item(NFD_NAME, &first, None),
        ];
        let got = get_checked(&repo, dir.path(), &items).unwrap();

        assert!(got.changed.is_empty());
        assert_eq!(read(&repo, "[ab]*.txt"), "[ab]*.txt one\n");
        assert_eq!(read(&repo, "-x.txt"), "-x.txt one\n");
        assert_eq!(read(&repo, ":(glob)b.txt"), ":(glob)b.txt one\n");
        assert_eq!(read(&repo, NFD_NAME), "nfd one\n");
        // What only a pattern would have reached is untouched.
        assert_eq!(read(&repo, "a.txt"), "a.txt two\n");
    }

    #[test]
    fn a_revision_that_cannot_be_read_writes_nothing() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        let first = repo.commit("dir/a.txt", b"one\n", "first");
        fs::write(repo.path().join("b.txt"), "local\n").unwrap();

        let items = [item("b.txt", &first, None), item("dir", &first, None)];
        let failed = get_checked(&repo, dir.path(), &items);

        assert!(matches!(failed, Err(GetError::Load(ref message)) if message.contains("folder")));
        assert_eq!(read(&repo, "b.txt"), "local\n");
        assert!(snapshots::list(dir.path()).is_empty());
    }

    #[test]
    fn a_write_between_the_check_and_the_get_is_reported_not_overwritten() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"a1\n", "first");
        let first = repo.commit("b.txt", b"b1\n", "second");
        repo.commit("a.txt", b"a2\n", "third");
        repo.commit("b.txt", b"b2\n", "fourth");
        let items = [item("a.txt", &first, None), item("b.txt", &first, None)];
        let checked = check(repo.path(), &items).unwrap();

        // While the `Get Revision` question is open: the check found it clean.
        fs::write(repo.path().join("b.txt"), "Devin, during the question\n").unwrap();
        let got = get(
            repo.path(),
            dir.path(),
            &items,
            &checked.head,
            &checked.seen,
        )
        .unwrap();

        assert_eq!(got.changed, ["b.txt"]);
        assert_eq!(got.written, ["a.txt"]);
        assert_eq!(read(&repo, "a.txt"), "a1\n");
        assert_eq!(read(&repo, "b.txt"), "Devin, during the question\n");
    }

    #[test]
    fn a_path_the_check_did_not_read_is_left() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        let first = repo.commit("a.txt", b"a1\n", "first");
        repo.commit("a.txt", b"a2\n", "second");

        let got = get(
            repo.path(),
            dir.path(),
            &[item("a.txt", &first, None)],
            &head(&repo),
            &[],
        )
        .unwrap();

        assert_eq!(got.changed, ["a.txt"]);
        assert_eq!(read(&repo, "a.txt"), "a2\n");
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect()
    }

    #[test]
    fn a_write_right_before_the_replace_is_left_and_reported() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        let first = repo.commit("a.txt", b"a1\n", "first");
        repo.commit("a.txt", b"a2\n", "second");

        let got = pausing(
            &repo,
            dir.path(),
            &[item("a.txt", &first, None)],
            |moment| {
                if *moment == Moment::BeforeReplace("a.txt".to_string()) {
                    fs::write(repo.path().join("a.txt"), "Devin, during the write\n").unwrap();
                }
            },
        )
        .unwrap();

        assert_eq!(got.changed, ["a.txt"]);
        assert!(got.written.is_empty());
        assert_eq!(read(&repo, "a.txt"), "Devin, during the write\n");
        assert!(
            leftovers(repo.path()).is_empty(),
            "{:?}",
            leftovers(repo.path())
        );
    }

    #[test]
    fn a_write_right_before_the_move_back_removes_is_left_and_reported() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        let first = repo.commit("old.txt", b"old\n", "first");
        repo.git(&["mv", "old.txt", "new.txt"]);
        repo.commit("new.txt", b"new\n", "rename");
        let items = [item("new.txt", &first, Some("old.txt"))];

        let got = pausing(&repo, dir.path(), &items, |moment| {
            if *moment == Moment::BeforeRemove("new.txt".to_string()) {
                fs::write(repo.path().join("new.txt"), "Devin, during the move\n").unwrap();
            }
        })
        .unwrap();

        assert_eq!(got.changed, ["new.txt"]);
        assert_eq!(read(&repo, "new.txt"), "Devin, during the move\n");
        // What was already written at the old path stays: it was not there before.
        assert_eq!(read(&repo, "old.txt"), "old\n");
    }

    // M1: the last comparison reads the whole file. A write while it reads, or right after it,
    // came after the copy: the Get must not overwrite it.
    #[test]
    fn a_write_while_the_file_is_compared_is_left_and_reported() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        let first = repo.commit("a.txt", b"a1\n", "first");
        repo.commit("a.txt", b"a2\n", "second");

        let got = pausing(
            &repo,
            dir.path(),
            &[item("a.txt", &first, None)],
            |moment| {
                if *moment == Moment::Compared("a.txt".to_string()) {
                    fs::write(repo.path().join("a.txt"), "Devin, while it was compared\n").unwrap();
                }
            },
        )
        .unwrap();

        assert_eq!(got.changed, ["a.txt"]);
        assert!(got.written.is_empty());
        assert_eq!(read(&repo, "a.txt"), "Devin, while it was compared\n");
        assert!(leftovers(repo.path()).is_empty());
    }

    // M1, the removal of a rename's local file: the same.
    #[test]
    fn a_write_while_the_moved_file_is_compared_is_not_removed() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        let first = repo.commit("old.txt", b"old\n", "first");
        repo.git(&["mv", "old.txt", "new.txt"]);
        repo.commit("new.txt", b"new\n", "rename");
        let items = [item("new.txt", &first, Some("old.txt"))];

        let got = pausing(&repo, dir.path(), &items, |moment| {
            if *moment == Moment::Compared("new.txt".to_string()) {
                fs::write(
                    repo.path().join("new.txt"),
                    "Devin, while it was compared\n",
                )
                .unwrap();
            }
        })
        .unwrap();

        assert_eq!(got.changed, ["new.txt"]);
        assert_eq!(read(&repo, "new.txt"), "Devin, while it was compared\n");
    }

    // M2: the revision's file was written over a local one at the old path, and the local file at
    // the new path changed and was left. What the old path held is in the copy: the old path is
    // reported written (so that the page offers Restore) and the new one left.
    #[test]
    fn a_written_target_with_a_left_move_back_is_reported_written() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        let first = repo.commit("old.txt", b"old\n", "first");
        repo.git(&["mv", "old.txt", "new.txt"]);
        repo.commit("new.txt", b"new\n", "rename");
        fs::write(repo.path().join("old.txt"), "mine, untracked\n").unwrap();
        let items = [item("new.txt", &first, Some("old.txt"))];

        let got = pausing(&repo, dir.path(), &items, |moment| {
            if *moment == Moment::BeforeRemove("new.txt".to_string()) {
                fs::write(repo.path().join("new.txt"), "Devin, during the move\n").unwrap();
            }
        })
        .unwrap();

        assert_eq!(got.written, ["old.txt"]);
        assert_eq!(got.changed, ["new.txt"]);
        assert_eq!(read(&repo, "old.txt"), "old\n");
        assert_eq!(read(&repo, "new.txt"), "Devin, during the move\n");
        assert_eq!(
            copied(dir.path(), &got.copy, "old.txt").as_deref(),
            Some("mine, untracked\n")
        );
    }

    // M2, when the removal fails: the old path was already written, and the interruption says so.
    #[test]
    fn a_failed_move_back_says_the_target_was_written() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        let first = repo.commit("old.txt", b"old\n", "first");
        fs::create_dir(repo.path().join("locked")).unwrap();
        repo.git(&["mv", "old.txt", "locked/new.txt"]);
        repo.commit("locked/new.txt", b"new\n", "rename");
        fs::write(repo.path().join("old.txt"), "mine, untracked\n").unwrap();
        let items = [item("locked/new.txt", &first, Some("old.txt"))];
        let checked = check(repo.path(), &items).unwrap();
        let locked = repo.path().join("locked");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o500)).unwrap();

        let failed = get(
            repo.path(),
            dir.path(),
            &items,
            &checked.head,
            &checked.seen,
        );
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

        let Err(GetError::Interrupted {
            copy,
            written,
            changed,
            ..
        }) = failed
        else {
            panic!("expected an interrupted Get: {failed:?}");
        };
        assert_eq!(written, ["old.txt"]);
        assert!(changed.is_empty());
        assert_eq!(read(&repo, "old.txt"), "old\n");
        assert_eq!(read(&repo, "locked/new.txt"), "new\n");
        assert_eq!(
            copied(dir.path(), &copy, "old.txt").as_deref(),
            Some("mine, untracked\n")
        );
    }

    // M6: a file removed between its `lstat` and the read of its content holds nothing; it does
    // not interrupt the Get.
    #[test]
    fn a_file_removed_after_its_lstat_has_no_stamp() {
        let repo = TempRepo::new();
        let path = repo.path().join("a.txt");
        fs::write(&path, "a\n").unwrap();
        let meta = fs::symlink_metadata(&path).unwrap();
        fs::remove_file(&path).unwrap();

        assert_eq!(stamp_with(&path, &meta).unwrap(), None);
    }

    // M6: a file removed after the check is left and reported; nothing else stops.
    #[test]
    fn a_file_removed_after_the_check_is_left_and_the_others_are_written() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"a1\n", "first");
        let first = repo.commit("b.txt", b"b1\n", "second");
        repo.commit("a.txt", b"a2\n", "third");
        repo.commit("b.txt", b"b2\n", "fourth");
        let items = [item("a.txt", &first, None), item("b.txt", &first, None)];
        let checked = check(repo.path(), &items).unwrap();
        fs::remove_file(repo.path().join("b.txt")).unwrap();

        let got = get(
            repo.path(),
            dir.path(),
            &items,
            &checked.head,
            &checked.seen,
        )
        .unwrap();

        assert_eq!(got.written, ["a.txt"]);
        assert_eq!(got.changed, ["b.txt"]);
        assert_eq!(read(&repo, "a.txt"), "a1\n");
        assert!(!repo.path().join("b.txt").exists());
    }

    // A link item empties its place before it makes the link. If the link cannot be made then (here
    // a file appears in that instant), what the place held is only in the copy: it is reported
    // written, so that the page offers Restore.
    #[test]
    fn a_link_that_cannot_be_made_after_its_place_was_emptied_is_reported_written() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("run.sh", b"#!/bin/sh\n", "script");
        std::os::unix::fs::symlink("run.sh", repo.path().join("link")).unwrap();
        let first = repo.commit("x.txt", b"x\n", "first");
        fs::remove_file(repo.path().join("link")).unwrap();
        repo.commit("link", b"a file now\n", "second");
        fs::write(repo.path().join("link"), "mine\n").unwrap();

        let failed = pausing(&repo, dir.path(), &[item("link", &first, None)], |moment| {
            if *moment == Moment::Removed("link".to_string()) {
                fs::write(repo.path().join("link"), "Devin, in that instant\n").unwrap();
            }
        });

        let Err(GetError::Interrupted { copy, written, .. }) = failed else {
            panic!("expected an interrupted Get: {failed:?}");
        };
        assert_eq!(written, ["link"]);
        assert_eq!(copied(dir.path(), &copy, "link").as_deref(), Some("mine\n"));
        assert_eq!(read(&repo, "link"), "Devin, in that instant\n");
    }

    // After a replace the path is expected to hold what was written, not what the disk holds a
    // moment later: a write in that instant is not replaced by a later item of the same Get.
    #[test]
    fn a_write_right_after_the_replace_is_not_overwritten_by_a_later_item() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        let first = repo.commit("a.txt", b"a1\n", "first");
        let second = repo.commit("a.txt", b"a2\n", "second");
        repo.commit("a.txt", b"a3\n", "third");
        let items = [item("a.txt", &first, None), item("a.txt", &second, None)];
        let mut replaced = 0;

        let got = pausing(&repo, dir.path(), &items, |moment| {
            if *moment == Moment::Replaced("a.txt".to_string()) {
                replaced += 1;
                if replaced == 1 {
                    fs::write(repo.path().join("a.txt"), "Devin, right after\n").unwrap();
                }
            }
        })
        .unwrap();

        assert_eq!(got.written, ["a.txt"]);
        assert_eq!(got.changed, ["a.txt"]);
        assert_eq!(read(&repo, "a.txt"), "Devin, right after\n");
    }

    #[test]
    fn a_failed_write_keeps_the_copy_and_says_what_was_written() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"a1\n", "first");
        let first = repo.commit("locked/b.txt", b"b1\n", "second");
        repo.commit("a.txt", b"a2\n", "third");
        repo.commit("locked/b.txt", b"b2\n", "fourth");
        let locked = repo.path().join("locked");
        let items = [
            item("a.txt", &first, None),
            item("locked/b.txt", &first, None),
        ];
        let checked = check(repo.path(), &items).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o500)).unwrap();

        let failed = get(
            repo.path(),
            dir.path(),
            &items,
            &checked.head,
            &checked.seen,
        );
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

        let Err(GetError::Interrupted {
            copy,
            written,
            changed,
            message,
        }) = failed
        else {
            panic!("expected an interrupted Get: {failed:?}");
        };
        assert_eq!(written, ["a.txt"]);
        assert!(changed.is_empty());
        assert!(!message.is_empty());
        assert_eq!(copied(dir.path(), &copy, "a.txt").as_deref(), Some("a2\n"));
        assert_eq!(read(&repo, "a.txt"), "a1\n");
        assert_eq!(read(&repo, "locked/b.txt"), "b2\n");
        let error = ApiError::from(GetError::Interrupted {
            message,
            copy: copy.clone(),
            written,
            changed,
        });
        assert_eq!(error.details.unwrap()["copy"], copy.as_str());
    }
}
