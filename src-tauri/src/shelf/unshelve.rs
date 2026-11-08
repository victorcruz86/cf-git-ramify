// Unshelving a whole list, the core of `ShelveChangesManager.unshelveChangeList` with
// `removeFilesFromShelf` (what a system unshelve does, `VcsShelveUtils.doSystemUnshelve`). The text
// part goes through `patch_apply` with the stored bases; the binary files are put back from their
// copies (`ShelvedBinaryFilePatch`). What was applied is recycled; what was not stays in the list.
//
// The conflicts that need the merge window are not written: they come back in `applied.conflicts`
// and the files stay in the list, so a later unshelve (the Shelf tab) can resolve them.
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::SystemTime;

use crate::branches::Repo;
use crate::commands::{self, ApiError};
use crate::git::{self, GitError};
use crate::patch::Patch;
use crate::patch_apply::{self, ApplyResult, ChangeKind, PathMapping, Status};
use crate::shelf::tab::ShelfFile;
use crate::shelf::{self, ShelvedBinary};
use crate::snapshots;
use crate::storage;

// `activity.name.unshelve` (V:1266): the reason of the copy taken before the binary files are put
// back. The text files take theirs in `patch_apply`.
const UNSHELVE_REASON: &str = "Unshelve";

// rama's own: why a binary file was not put back.
fn binary_changed(path: &str) -> String {
    format!("\"{path}\" changed on disk while it was shelved: it was not unshelved")
}

fn binary_exists(path: &str) -> String {
    format!("\"{path}\" already exists: it was not unshelved")
}

fn binary_moved_changed(path: &str) -> String {
    format!("\"{path}\" changed while it was unshelved: it was left as it is")
}

fn binary_head_changed(path: &str) -> String {
    format!("\"{path}\" changed in HEAD since it was shelved: it was not unshelved")
}

// What one list gave back. `written` are the repository paths that hold the list's changes now (for
// the changelist they go to); `skipped` are the new files whose place was taken meanwhile;
// `remaining` says that something stays in the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unshelved {
    pub applied: ApplyResult,
    pub binaries_failed: Vec<String>,
    pub skipped: Vec<String>,
    pub written: Vec<String>,
    pub remaining: bool,
}

fn io(error: std::io::Error) -> GitError {
    GitError::Io(error.to_string())
}

// The planned file a conflict belongs to: by its target, or by the file it moves from.
fn in_conflict(result: &ApplyResult, path: &str, before: Option<&str>) -> bool {
    result.conflicts.iter().any(|conflict| {
        conflict.path == path
            || conflict.moved_to.as_deref() == Some(path)
            || Some(conflict.path.as_str()) == before
    })
}

// The whole list `name`, applied over the repository as it is. Nothing that is there is overwritten:
// a new file whose place is taken, a change that does not fit and a conflict stay in the list.
// `into` is the changelist the written files go to (`targetChangeList`), if it still exists.
pub fn unshelve(
    repo: &Repo,
    name: &str,
    into: Option<&str>,
    now: SystemTime,
) -> Result<Unshelved, GitError> {
    let data = repo.data;
    let list = shelf::find(data, name).map_err(io)?;
    let patch = shelf::load_patch(data, name).map_err(io)?;
    let mapping = PathMapping::default();
    let plan = patch_apply::plan(repo.path, &patch, &mapping);
    if let Some(error) = plan.read_error {
        return Err(GitError::Io(error));
    }
    let selected: Vec<String> = plan.files.iter().map(|file| file.path.clone()).collect();
    let expected: BTreeMap<String, String> = plan
        .files
        .iter()
        .flat_map(|file| file.hashes.clone())
        .collect();
    let applied = patch_apply::apply(repo, &patch, &mapping, &selected, &[], &expected)?;

    let mut remaining_files = Vec::new();
    let mut skipped = Vec::new();
    let mut written = Vec::new();
    for file in &plan.files {
        let taken = file.kind == ChangeKind::Added && file.status == Status::Exists;
        let failed = applied.failed.contains(&file.path);
        if taken || failed || in_conflict(&applied, &file.path, file.before.as_deref()) {
            remaining_files.push(patch.files[file.index].clone());
            if taken {
                skipped.push(file.path.clone());
            }
        } else {
            written.push(file.path.clone());
            written.extend(file.before.clone().filter(|before| *before != file.path));
        }
    }

    let mut remaining_binaries: Vec<ShelvedBinary> = Vec::new();
    let mut binaries_failed = Vec::new();
    let base_ids = shelf::base_ids(&list);
    for binary in &list.binaries {
        match put_back(repo, binary, &base_ids, None, HeadChanged::Refuse) {
            Ok(paths) => written.extend(paths),
            Err(message) => {
                binaries_failed.push(message);
                remaining_binaries.push(binary.clone());
            }
        }
    }

    let remaining = !remaining_files.is_empty() || !remaining_binaries.is_empty();
    let nothing_applied = remaining_files.len() == patch.files.len()
        && remaining_binaries.len() == list.binaries.len();
    if !remaining {
        shelf::recycle(data, name, now).map_err(io)?;
    } else if !nothing_applied {
        let rest = Patch {
            files: remaining_files,
            ..patch.clone()
        };
        shelf::save_remaining_and_recycle_applied(data, name, &rest, &remaining_binaries, now)
            .map_err(io)?;
    }
    if let Some(id) = into {
        move_into(data, &written, id);
    }
    Ok(Unshelved {
        applied,
        binaries_failed,
        skipped,
        written,
        remaining,
    })
}

// `PatchApplier` with a target list: what was written goes there. A list that no longer exists, or
// changelists that cannot be read, leave the files where the next refresh puts them.
fn move_into(data: &Path, paths: &[String], id: &str) {
    if paths.is_empty() {
        return;
    }
    let _ = commands::edit_changelists(data, |lists| {
        if lists.lists.iter().any(|list| list.id == id) {
            lists.move_paths(paths, id)?;
        }
        Ok(())
    });
}

// Whether git sees anything of these paths changed, or new, on disk or in the index.
fn touched_locally(repo: &Path, paths: &[String]) -> Result<bool, String> {
    let head = ["status", "--porcelain=v1", "-z", "--untracked-files=all"];
    git::git_with_paths(repo, false, &head, paths)
        .map(|output| !output.is_empty())
        .map_err(|error| ApiError::from(error).message)
}

// The repository paths a binary file reads or writes: its before path, then its after path.
pub(crate) fn binary_paths(binary: &ShelvedBinary) -> Vec<String> {
    binary
        .before
        .iter()
        .chain(
            binary
                .after
                .iter()
                .filter(|after| binary.before.as_ref() != Some(*after)),
        )
        .cloned()
        .collect()
}

// The path a binary file is shown with: the after path, else the before one.
pub(crate) fn binary_shown(binary: &ShelvedBinary) -> String {
    binary
        .after
        .as_ref()
        .or(binary.before.as_ref())
        .cloned()
        .unwrap_or_default()
}

// Whether a binary file whose before file HEAD changed since the list was made may be written
// over: the Unshelve dialog's per-row `Overwrite` (rama's own; IntelliJ writes the shelved version
// without asking).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HeadChanged {
    Refuse,
    Overwrite,
}

// Why a binary file cannot go back now, if it cannot: a path outside the repository, a file that
// git sees changed, a before file that HEAD changed since the list was made (`base_ids`, when the
// list recorded it) unless `head_changed` says to overwrite it, or a new file whose place is taken.
pub(crate) fn binary_problem(
    root: &Path,
    binary: &ShelvedBinary,
    base_ids: &BTreeMap<String, String>,
    head_changed: HeadChanged,
) -> Result<(), String> {
    let paths = binary_paths(binary);
    for path in &paths {
        snapshots::check_path(root, path).map_err(|error| error.to_string())?;
    }
    let shown = binary_shown(binary);
    if touched_locally(root, &paths)? {
        return Err(binary_changed(&shown));
    }
    if let Some(before) = binary
        .before
        .as_ref()
        .filter(|_| head_changed == HeadChanged::Refuse)
    {
        if let Some(id) = base_ids.get(before) {
            if shelf::head_blob(root, before).as_ref() != Some(id) {
                return Err(binary_head_changed(&shown));
            }
        }
    }
    let created = binary
        .after
        .as_ref()
        .filter(|after| binary.before.as_ref() != Some(*after));
    if let Some(after) = created {
        if root.join(after).symlink_metadata().is_ok() {
            return Err(binary_exists(after));
        }
    }
    Ok(())
}

// `ShelvedBinaryFilePatch`: the copy goes back whole. Only over a file nobody changed: one that
// differs from what git has, whose HEAD version is no longer the one it was shelved over (unless
// `head_changed` says to overwrite it), or a new file whose place is taken, stays in the list. With
// `expected` (the hashes a plan read), a file that changed since then is not written either. The
// recoverable copy comes first. Returns the paths that now hold the change.
pub(crate) fn put_back(
    repo: &Repo,
    binary: &ShelvedBinary,
    base_ids: &BTreeMap<String, String>,
    expected: Option<&BTreeMap<String, String>>,
    head_changed: HeadChanged,
) -> Result<Vec<String>, String> {
    put_back_watching(repo, binary, base_ids, expected, head_changed, &mut |_| {})
}

// When `put_back_watching` lets the tests write as another program: right before the target is
// written, and right before the source of a move is removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PutBackMoment {
    Write,
    Remove,
}

// `put_back`, with `watch` run at each `PutBackMoment`.
pub(crate) fn put_back_watching(
    repo: &Repo,
    binary: &ShelvedBinary,
    base_ids: &BTreeMap<String, String>,
    expected: Option<&BTreeMap<String, String>>,
    head_changed: HeadChanged,
    watch: &mut dyn FnMut(PutBackMoment),
) -> Result<Vec<String>, String> {
    let root = repo.path;
    binary_problem(root, binary, base_ids, head_changed)?;
    let paths = binary_paths(binary);
    let shown = binary_shown(binary);
    let created = binary
        .after
        .as_ref()
        .filter(|after| binary.before.as_ref() != Some(*after));
    let bytes = match (&binary.after, &binary.shelved) {
        (Some(_), Some(copy)) => Some(fs::read(copy).map_err(|error| error.to_string())?),
        (Some(after), None) => return Err(binary_changed(after)),
        (None, _) => None,
    };
    // As `patch_apply`: the copy first, then the disk must be what the copy holds and what the
    // plan read. Anything newer would not be recoverable, or was not what the user chose.
    let text = |error: std::io::Error| error.to_string();
    let snapshot = snapshots::create(repo.data, root, &paths, UNSHELVE_REASON).map_err(text)?;
    let stored = snapshots::stored_files(repo.data, &snapshot).map_err(text)?;
    let mut copied = BTreeMap::new();
    for path in &paths {
        // An absent file has an entry and no slot, as `patch_apply` reads it.
        let held = stored.get(path).and_then(|file| fs::read(file).ok());
        let held = patch_apply::hash_of(held.as_deref());
        let now = patch_apply::hash_at(root, path).map_err(text)?;
        let planned = expected.is_none_or(|expected| expected.get(path) == Some(&held));
        if now != held || !planned {
            return Err(binary_changed(&shown));
        }
        copied.insert(path.clone(), held);
    }
    let index = |head: &[&str], paths: &[String]| -> Result<(), String> {
        if paths.is_empty() {
            return Ok(());
        }
        git::git_with_paths(root, true, head, paths)
            .map(|_| ())
            .map_err(|error| ApiError::from(error).message)
    };
    let mut added = Vec::new();
    let mut removed = Vec::new();
    if let (Some(after), Some(bytes)) = (&binary.after, &bytes) {
        watch(PutBackMoment::Write);
        // The target too is written only if it is still what the copy holds.
        let now = patch_apply::hash_at(root, after).map_err(text)?;
        if copied.get(after) != Some(&now) {
            return Err(binary_moved_changed(after));
        }
        let full = root.join(after);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let permissions = full.symlink_metadata().ok().map(|meta| meta.permissions());
        storage::write_atomic_with(&full, bytes, permissions).map_err(|error| error.to_string())?;
        if created.is_some() {
            added.push(after.clone());
        }
    }
    if let Some(before) = binary
        .before
        .as_ref()
        .filter(|before| binary.after.as_ref() != Some(*before))
    {
        watch(PutBackMoment::Remove);
        // `Writer::unchanged` of `patch_apply`: the source goes only if it is still what the copy
        // holds. Otherwise it stays, and the binary stays in the list.
        let now = patch_apply::hash_at(root, before).map_err(text)?;
        if copied.get(before) != Some(&now) {
            index(&["add"], &added)?;
            return Err(binary_moved_changed(before));
        }
        fs::remove_file(root.join(before)).map_err(|error| error.to_string())?;
        removed.push(before.clone());
    }
    index(&["rm", "--cached", "-q", "--ignore-unmatch"], &removed)?;
    index(&["add"], &added)?;
    Ok(paths)
}

// `Remove successfully applied files from the shelf` after an unshelve of the Shelf tab
// (`updateListAfterUnshelve`): `applied` are the files of the list that went through, merged
// conflicts included. Everything applied recycles the list (again, dated now, if it was already);
// a part goes to a recycled copy and the list keeps the rest, unless the list is recycled already.
// Nothing applied changes nothing.
pub fn recycle_applied(
    data: &Path,
    name: &str,
    applied: &[ShelfFile],
    now: SystemTime,
) -> Result<(), GitError> {
    let list = shelf::find(data, name).map_err(io)?;
    let patch = shelf::load_patch(data, name).map_err(io)?;
    let files: Vec<_> = patch
        .files
        .iter()
        .filter(|file| !applied.contains(&ShelfFile::of_patch(file)))
        .cloned()
        .collect();
    let binaries: Vec<ShelvedBinary> = list
        .binaries
        .iter()
        .filter(|binary| !applied.contains(&ShelfFile::of_binary(binary)))
        .cloned()
        .collect();
    if files.len() == patch.files.len() && binaries.len() == list.binaries.len() {
        return Ok(());
    }
    let rest = Patch { files, ..patch };
    shelf::save_remaining_and_recycle_applied(data, name, &rest, &binaries, now).map_err(io)?;
    Ok(())
}
