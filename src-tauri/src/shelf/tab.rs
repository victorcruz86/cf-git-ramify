// What the Shelf tab reads and does with the store (`ShelvedChangesViewManager`,
// `ShelveChangesManager.deleteShelves`, `restoreList`, `importChangeLists`, `gatherPatchFiles`,
// `importFilePatches`, `DiffShelvedChangesActionProvider`). Read at the tag idea/241.14494.240.
//
// A file of a list is named by the shelf's own paths (`ShelfFile`), never by a position: the
// list may change between the moment the tab read it and the moment a command acts.
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::diff::{self, Side};
use crate::patch::{self, FilePatch, LineKind, Patch, PatchSource, TextFilePatch};
use crate::patch_apply::{self, PatchInput, PathMapping};
use crate::shelf::{self, find, millis, ShelvedBinary, ShelvedList};
use crate::snapshots;

// `shelve.loading.patch.error` (V:479).
fn loading_error(reason: &str) -> String {
    format!("Cannot load patch file: {reason}")
}

// `changes.error.can.t.show.diff.for` (V:1046) and `changes.error.content.for.0.was.removed`
// (V:1045).
fn cannot_show(path: &str) -> String {
    format!("Cannot show diff for '{path}'")
}

fn removed(path: &str) -> String {
    format!("Content for '{path}' was removed")
}

// DiffBundle `merge.version.title.base` and `.current`, and `shelve.shelved.version` (V:456).
const BASE_VERSION: &str = "Base version";
const CURRENT_VERSION: &str = "Current version";
const SHELVED_VERSION: &str = "Shelved version";

// `PatchFileType`: the extensions of a patch file.
const PATCH_EXTENSIONS: [&str; 2] = ["patch", "diff"];

fn io_text(error: io::Error) -> String {
    error.to_string()
}

// One file of a shelved list, by the paths the shelf has for it (`ShelvedChange` and
// `ShelvedBinaryFile`): `before: None` is a new file and `after: None` a deleted one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShelfFile {
    pub before: Option<String>,
    pub after: Option<String>,
    pub binary: bool,
}

impl ShelfFile {
    pub fn of_patch(file: &FilePatch) -> Self {
        let (before, after) = match file {
            FilePatch::Text(text) => (text.before.clone(), text.after.clone()),
            FilePatch::Binary(binary) => (binary.before.clone(), binary.after.clone()),
        };
        Self {
            before,
            after,
            binary: false,
        }
    }

    pub fn of_binary(binary: &ShelvedBinary) -> Self {
        Self {
            before: binary.before.clone(),
            after: binary.after.clone(),
            binary: true,
        }
    }

    fn path(&self) -> &str {
        self.after
            .as_deref()
            .or(self.before.as_deref())
            .unwrap_or_default()
    }

    // The status letter of `ChangedFile`: added, deleted, renamed or modified.
    fn status(&self) -> &'static str {
        match (&self.before, &self.after) {
            (None, _) => "A",
            (_, None) => "D",
            (Some(before), Some(after)) if before != after => "R",
            _ => "M",
        }
    }
}

// A change of a list as the tab shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShelfChange {
    pub file: ShelfFile,
    pub path: String,
    pub old_path: Option<String>,
    pub status: String,
}

impl ShelfChange {
    fn of(file: ShelfFile) -> Self {
        let path = file.path().to_string();
        let old_path = file.before.clone().filter(|before| *before != path);
        Self {
            status: file.status().to_string(),
            path,
            old_path,
            file,
        }
    }
}

// A list with its changes, or with the error that its patch gave (`getChangesLoadingError`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShelfEntry {
    pub list: ShelvedList,
    pub changes: Vec<ShelfChange>,
    pub error: Option<String>,
}

// Every list of the store, newest first, with its changes: the patch's, then the binary files.
pub fn entries(data: &Path) -> Vec<ShelfEntry> {
    shelf::list(data)
        .into_iter()
        .map(|list| {
            let binaries = list
                .binaries
                .iter()
                .map(|binary| ShelfChange::of(ShelfFile::of_binary(binary)));
            match shelf::load_patch(data, &list.name) {
                Ok(patch) => ShelfEntry {
                    changes: patch
                        .files
                        .iter()
                        .map(|file| ShelfChange::of(ShelfFile::of_patch(file)))
                        .chain(binaries)
                        .collect(),
                    error: None,
                    list,
                },
                Err(error) => ShelfEntry {
                    changes: binaries.collect(),
                    error: Some(loading_error(&error.to_string())),
                    list,
                },
            }
        })
        .collect()
}

// A list as the applier reads it: its patch with the stored bases, its binary files and what
// they were shelved over; only `files` when given.
pub struct ShelfSource {
    pub patch: Patch,
    pub binaries: Vec<ShelvedBinary>,
    pub base_ids: BTreeMap<String, String>,
}

pub fn source(data: &Path, name: &str, files: Option<&[ShelfFile]>) -> Result<ShelfSource, String> {
    let list = find(data, name).map_err(io_text)?;
    let mut patch = shelf::load_patch(data, name).map_err(io_text)?;
    let mut binaries = list.binaries.clone();
    if let Some(files) = files {
        patch
            .files
            .retain(|file| files.contains(&ShelfFile::of_patch(file)));
        binaries.retain(|binary| files.contains(&ShelfFile::of_binary(binary)));
    }
    Ok(ShelfSource {
        patch,
        binaries,
        base_ids: shelf::base_ids(&list),
    })
}

//
// Delete, Undo and Restore
//

// The files chosen in one list.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListFiles {
    pub name: String,
    pub files: Vec<ShelfFile>,
}

// A list that the Undo of `Shelf Deletion` gives back, with the date it had.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Restorable {
    pub name: String,
    pub date_ms: u64,
}

// What Delete did: the lists to give back, and what the notification counts (the files chosen
// outside the whole lists, the whole lists and the first one's description).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Deleted {
    pub restorable: Vec<Restorable>,
    pub files: usize,
    pub lists: usize,
    pub first: Option<String>,
    pub errors: Vec<String>,
}

// `deleteShelves`: whole lists become deleted (or are erased if they were already); the files
// chosen in other lists go to a deleted copy, or the list itself becomes deleted when nothing is
// left. No question is asked.
pub fn delete(
    data: &Path,
    lists: &[String],
    files: &[ListFiles],
    now: SystemTime,
) -> io::Result<Deleted> {
    let mut restorable = Vec::new();
    let mut first = None;
    let mut errors = Vec::new();
    let mut deleted_lists = 0;
    // What fails is said, and what was done before and after it stays done, with its Undo. The
    // message counts only the lists really deleted (or erased).
    for name in lists {
        let deleted = find(data, name).and_then(|list| {
            let done = shelf::delete(data, name, now)?.map(|_| Restorable {
                name: list.name,
                date_ms: list.date_ms,
            });
            Ok((list.description, done))
        });
        match deleted {
            Ok((description, done)) => {
                deleted_lists += 1;
                first.get_or_insert(description);
                restorable.extend(done);
            }
            Err(error) => errors.push(format!("{name}: {error}")),
        }
    }
    let mut count = 0;
    for chosen in files.iter().filter(|chosen| !lists.contains(&chosen.name)) {
        match delete_files(data, chosen, now) {
            Ok((taken, done)) => {
                count += taken;
                restorable.extend(done);
            }
            Err(error) => errors.push(error),
        }
    }
    Ok(Deleted {
        restorable,
        files: count,
        lists: deleted_lists,
        first,
        errors,
    })
}

// `removeChangesFromChangeList`: how many files were taken, and the list that holds them now.
fn delete_files(
    data: &Path,
    chosen: &ListFiles,
    now: SystemTime,
) -> Result<(usize, Option<Restorable>), String> {
    let list = find(data, &chosen.name).map_err(io_text)?;
    // `shelve.delete.files.from.changelist.error` (V:475).
    let failed =
        |error: io::Error| format!("Cannot delete files from {}: {error}", list.description);
    let patch = shelf::load_patch(data, &chosen.name).map_err(failed)?;
    let remaining = Patch {
        files: patch
            .files
            .iter()
            .filter(|file| !chosen.files.contains(&ShelfFile::of_patch(file)))
            .cloned()
            .collect(),
        ..patch.clone()
    };
    let binaries: Vec<ShelvedBinary> = list
        .binaries
        .iter()
        .filter(|binary| !chosen.files.contains(&ShelfFile::of_binary(binary)))
        .cloned()
        .collect();
    let taken = patch.files.len() - remaining.files.len() + list.binaries.len() - binaries.len();
    if taken == 0 {
        return Ok((0, None));
    }
    let copy =
        shelf::save_remaining_and_delete_others(data, &chosen.name, &remaining, &binaries, now)
            .map_err(failed)?;
    let done = match copy {
        Some(copy) => Some(Restorable {
            name: copy.name,
            date_ms: list.date_ms,
        }),
        // The whole list was taken: it is the deleted one.
        None if !list.deleted && find(data, &chosen.name).is_ok_and(|now| now.deleted) => {
            Some(Restorable {
                name: list.name.clone(),
                date_ms: list.date_ms,
            })
        }
        None => None,
    };
    Ok((taken, done))
}

// `UndoShelfDeletionAction`: each list back with its date. Returns how many could not come back,
// because they were erased meanwhile.
pub fn undo_delete(data: &Path, lists: &[Restorable]) -> io::Result<usize> {
    let mut gone = 0;
    for restorable in lists {
        match find(data, &restorable.name) {
            Ok(list) if list.deleted => {
                shelf::restore(data, &restorable.name, restorable.date_ms)?;
            }
            _ => gone += 1,
        }
    }
    Ok(gone)
}

// `RestoreShelvedChange`: the deleted lists come back, dated now.
pub fn restore(data: &Path, names: &[String], now: SystemTime) -> io::Result<()> {
    let date = now
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64);
    for name in names {
        let list = find(data, name)?;
        if list.deleted {
            shelf::restore(data, name, date)?;
        }
    }
    Ok(())
}

// Ramify's own recovery of what the store erased (`<data>/shelf-erased/`, kept 30 days): a folder
// of it moved by hand into `<data>/shelf-restore/` goes back into the store when the repository
// opens, before the cleanup.
const RESTORE_DIR: &str = "shelf-restore";

fn not_one_list() -> io::Error {
    shelf::invalid("The folder does not hold one shelved changelist")
}

// What `restore_requested` brought back, and what it left where it was, with why.
#[derive(Debug, Default)]
pub struct RestoredErased {
    pub lists: Vec<ShelvedList>,
    pub errors: Vec<String>,
}

// Each folder of `<data>/shelf-restore/`, as `shelf-erased/` keeps a list (its folder `<name>/`
// and `<name>.xml`), back into the store.
pub fn restore_requested(data: &Path, now: SystemTime) -> RestoredErased {
    let mut done = RestoredErased::default();
    let base = data.join(RESTORE_DIR);
    let Ok(entries) = fs::read_dir(&base) else {
        return done;
    };
    let mut folders: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect();
    folders.sort();
    for folder in folders {
        match restore_erased(data, &folder, now) {
            Ok(list) => done.lists.push(list),
            Err(error) => done.errors.push(format!("{}: {error}", folder.display())),
        }
    }
    done
}

// `restore` (`RestoreShelvedChange`) for a list the store erased: its folder and its XML go back
// into the store, and the list is no longer deleted and is dated now, so that the weekly cleanup
// does not erase it again. Nothing moves when a list has that name or the folder does not hold
// one list.
fn restore_erased(data: &Path, erased: &Path, now: SystemTime) -> io::Result<ShelvedList> {
    let _guard = shelf::lock();
    let xmls: Vec<String> = fs::read_dir(erased)?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|file| file.strip_suffix(".xml").map(str::to_string))
        .collect();
    let [name] = xmls.as_slice() else {
        return Err(not_one_list());
    };
    let folder = erased.join(name);
    if !shelf::valid_name(name) || !folder.symlink_metadata().is_ok_and(|meta| meta.is_dir()) {
        return Err(not_one_list());
    }
    let root = shelf::shelf_root(data);
    let (target, xml) = (root.join(name), shelf::xml_file(&root, name));
    if target.symlink_metadata().is_ok() || xml.symlink_metadata().is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("A shelved changelist named {name} exists"),
        ));
    }
    fs::create_dir_all(&root)?;
    fs::rename(&folder, &target)?;
    if let Err(error) = fs::rename(shelf::xml_file(erased, name), &xml) {
        let _ = fs::rename(&target, &folder);
        return Err(error);
    }
    let Ok(mut list) = find(data, name) else {
        // Not a list of this store (its XML names another place): it goes back where it was.
        let _ = fs::rename(&xml, shelf::xml_file(erased, name));
        let _ = fs::rename(&target, &folder);
        return Err(not_one_list());
    };
    list.deleted = false;
    list.date_ms = millis(now);
    shelf::store(data, &list)?;
    let _ = fs::remove_dir(erased);
    Ok(list)
}

//
// Import Patches… and Import to Shelf
//

fn is_patch_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            PATCH_EXTENSIONS
                .iter()
                .any(|known| known.eq_ignore_ascii_case(extension))
        })
}

// `gatherPatchFiles`: the chosen files that are patches, and the patches inside the chosen
// folders, at any depth. A link to a folder is not followed. Nothing is written.
pub fn find_patches(chosen: &[PathBuf]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut queue: std::collections::VecDeque<PathBuf> = chosen.iter().cloned().collect();
    while let Some(path) = queue.pop_front() {
        let Ok(meta) = path.symlink_metadata() else {
            continue;
        };
        if meta.is_dir() {
            let Ok(entries) = fs::read_dir(&path) else {
                continue;
            };
            let mut children: Vec<PathBuf> = entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .collect();
            children.sort();
            queue.extend(children);
        } else if is_patch_file(&path) && path.is_file() {
            found.push(path);
        }
    }
    found
}

// What Import Patches… made, and the files that could not be read, each with why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedPatches {
    pub lists: Vec<ShelvedList>,
    pub errors: Vec<String>,
}

// What the user reads of a patch that cannot be read (`shelf::patch_error`).
pub(crate) fn patch_error_text(error: patch::PatchError) -> String {
    shelf::patch_error(error).to_string()
}

// `importChangeLists`: one list per patch file, named after the file without its extension with
// `_` as spaces, dated as the file, holding the file as it is. A file with no change gives no
// list.
pub fn import_patches(data: &Path, files: &[PathBuf]) -> ImportedPatches {
    let mut lists = Vec::new();
    let mut errors = Vec::new();
    for file in files {
        let description = file
            .file_stem()
            .map(|stem| stem.to_string_lossy().replace('_', " "))
            .unwrap_or_default();
        let made = (|| -> Result<Option<ShelvedList>, String> {
            let meta = fs::metadata(file).map_err(io_text)?;
            if !meta.is_file() {
                return Err(format!("{} is not a file", file.display()));
            }
            let bytes = fs::read(file).map_err(io_text)?;
            let read = patch::read(&bytes).map_err(patch_error_text)?;
            if read.files.is_empty() {
                return Ok(None);
            }
            let date = meta.modified().map(millis).unwrap_or_default();
            shelf::add_patch_file(data, &description, &bytes, date)
                .map(Some)
                .map_err(io_text)
        })();
        match made {
            Ok(Some(list)) => lists.push(list),
            Ok(None) => {}
            Err(error) => errors.push(format!("{}: {error}", file.display())),
        }
    }
    ImportedPatches { lists, errors }
}

// `ImportToShelfExecutor`: the files the dialog included, at the paths it maps them to, with the
// patch's bases, in a new list named after the patch file.
pub fn import_to_shelf(
    data: &Path,
    repo: &Path,
    source: &PatchInput,
    mapping: &PathMapping,
    selected: &[String],
) -> Result<ShelvedList, String> {
    let PatchInput::File { path } = source else {
        return Err("Only a patch file can be imported to the shelf".to_string());
    };
    let description = Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut patch = patch_apply::mapped_patch(repo, source, mapping, selected)
        .map_err(|error| crate::commands::ApiError::from(error).message)?;
    // `ImportToShelfExecutor` takes the text patches (`TextFilePatchInProgress`) only.
    patch
        .files
        .retain(|file| matches!(file, FilePatch::Text(_)));
    if patch.files.is_empty() {
        return Err("Nothing to import".to_string());
    }
    shelf::create(data, repo, &description, &patch, &[], false).map_err(io_text)
}

//
// Show Diff, Compare with Local, Create Patch
//

// The two texts of a diff and their titles.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShelfSides {
    pub left: Side,
    pub right: Side,
    pub titles: [String; 2],
}

fn side_of(bytes: &[u8]) -> Side {
    if bytes.len() as u64 > diff::MAX_DIFF_BYTES {
        return Side::TooLarge {
            bytes: bytes.len() as u64,
        };
    }
    if diff::looks_binary(bytes) {
        return Side::Binary;
    }
    Side::Text {
        content: String::from_utf8_lossy(bytes).into_owned(),
    }
}

// The file of the repository, if it is a regular file inside it.
fn local(repo: &Path, path: &str) -> Option<Vec<u8>> {
    snapshots::check_path(repo, path).ok()?;
    let full = repo.join(path);
    full.symlink_metadata()
        .ok()
        .filter(|meta| meta.is_file())
        .and_then(|_| fs::read(full).ok())
}

// `getSingleHunkPatchText`: what a new file adds, or what a deleted one had.
fn single_hunk_text(file: &TextFilePatch, kind: LineKind) -> Vec<u8> {
    let mut text = Vec::new();
    for line in file.hunks.iter().flat_map(|hunk| &hunk.lines) {
        if line.kind == kind || line.kind == LineKind::Context {
            text.extend_from_slice(&line.text);
            if !line.no_newline {
                text.push(b'\n');
            }
        }
    }
    text
}

// A side of a change: nothing (a new or a deleted file), or its bytes.
type Contents = Option<Vec<u8>>;

// The before and after contents of a text change of the shelf: the stored base and the base with
// the hunks, else the local file and the local file with the hunks.
fn text_contents(repo: &Path, file: &TextFilePatch) -> Result<(Contents, Contents), String> {
    let path = file
        .after
        .as_deref()
        .or(file.before.as_deref())
        .unwrap_or_default();
    match (&file.before, &file.after) {
        (None, _) => Ok((None, Some(single_hunk_text(file, LineKind::Added)))),
        (Some(_), None) => Ok((
            Some(
                file.base
                    .clone()
                    .unwrap_or_else(|| single_hunk_text(file, LineKind::Removed)),
            ),
            None,
        )),
        (Some(before), Some(_)) => {
            let base = file.base.clone().or_else(|| local(repo, before));
            let base = base.ok_or_else(|| cannot_show(path))?;
            let patched = patch_apply::apply_hunks(&base, &file.hunks)
                .or_else(|| {
                    local(repo, before)
                        .and_then(|local| patch_apply::apply_hunks(&local, &file.hunks))
                })
                .ok_or_else(|| cannot_show(path))?;
            Ok((Some(base), Some(patched)))
        }
    }
}

fn find_file<'a>(patch: &'a Patch, file: &ShelfFile) -> Option<&'a TextFilePatch> {
    patch.files.iter().find_map(|entry| match entry {
        FilePatch::Text(text) if ShelfFile::of_patch(entry) == *file => Some(text),
        _ => None,
    })
}

fn find_binary<'a>(list: &'a ShelvedList, file: &ShelfFile) -> Option<&'a ShelvedBinary> {
    list.binaries
        .iter()
        .find(|binary| ShelfFile::of_binary(binary) == *file)
}

// `Show Diff` (the shelved version against its base) and `Compare with Local` (against the file
// on disk) of one file of a list.
pub fn sides(
    data: &Path,
    repo: &Path,
    name: &str,
    file: &ShelfFile,
    with_local: bool,
) -> Result<ShelfSides, String> {
    let list = find(data, name).map_err(io_text)?;
    let path = file.path().to_string();
    let local_side = || {
        let candidates = [file.after.as_deref(), file.before.as_deref()];
        candidates
            .into_iter()
            .flatten()
            .find_map(|path| local(repo, path))
            .map_or(Side::Absent, |bytes| side_of(&bytes))
    };
    let side = |bytes: Option<Vec<u8>>| bytes.map_or(Side::Absent, |bytes| side_of(&bytes));
    let (left, right) = if file.binary {
        let binary = find_binary(&list, file).ok_or_else(|| cannot_show(&path))?;
        let copy = match &binary.shelved {
            Some(copy) => Some(fs::read(copy).map_err(|_| removed(&path))?),
            None if with_local => None,
            None => return Err(removed(&path)),
        };
        let left = if with_local {
            local_side()
        } else {
            Side::Absent
        };
        (left, side(copy))
    } else {
        let patch = shelf::load_patch(data, name).map_err(io_text)?;
        let text = find_file(&patch, file).ok_or_else(|| cannot_show(&path))?;
        let (before, after) = text_contents(repo, text)?;
        let left = if with_local {
            local_side()
        } else {
            side(before)
        };
        (left, side(after))
    };
    let first = if with_local {
        CURRENT_VERSION
    } else {
        BASE_VERSION
    };
    Ok(ShelfSides {
        left,
        right,
        titles: [first.to_string(), SHELVED_VERSION.to_string()],
    })
}

// `Create Patch...` and `Copy as Patch to Clipboard` of the tab: each chosen file as its before
// and shelved contents (`ShelvedWrapper.getChangeWithLocal`).
pub fn patch_sources(
    data: &Path,
    repo: &Path,
    chosen: &[ListFiles],
) -> Result<Vec<PatchSource>, String> {
    let mut sources = Vec::new();
    for files in chosen {
        let list = find(data, &files.name).map_err(io_text)?;
        let patch = shelf::load_patch(data, &files.name).map_err(io_text)?;
        for file in &files.files {
            for path in [&file.before, &file.after].into_iter().flatten() {
                snapshots::check_path(repo, path).map_err(io_text)?;
            }
            let path = file.path().to_string();
            let (before, after, binary) = if file.binary {
                let shelved = find_binary(&list, file).ok_or_else(|| cannot_show(&path))?;
                let after = match &shelved.shelved {
                    Some(copy) => Some(fs::read(copy).map_err(|_| removed(&path))?),
                    None => None,
                };
                let before = file
                    .before
                    .as_deref()
                    .and_then(|before| local(repo, before));
                (before, after, true)
            } else {
                let text = find_file(&patch, file).ok_or_else(|| cannot_show(&path))?;
                let (before, after) = text_contents(repo, text)?;
                (before, after, false)
            };
            sources.push(PatchSource {
                before: file.before.clone().zip(before),
                after: file.after.clone().zip(after),
                before_rev: None,
                binary,
            });
        }
    }
    Ok(sources)
}

#[cfg(test)]
mod tests {
    use super::*;
    // Finding 6: such a binary offers `Overwrite` in the dialog, and with it chosen the shelved
    // version is written over the one HEAD brought, after the copy that keeps the latter.
    #[test]
    fn a_binary_changed_in_head_can_be_overwritten_from_the_dialog() {
        let harness = RepoHarness::new();
        harness.repo.commit("c.bin", &[0, 1], "first");
        let list = shelve(&harness, "Work", &[], &[("c.bin", Some(&[0, 9]))]);
        harness.repo.commit("c.bin", &[0, 3], "upstream c");
        let input = shelf_input(&list.name);
        let plan = plan_input(
            harness.path(),
            harness.data.path(),
            &input,
            &PathMapping::default(),
        )
        .unwrap();
        let file = &plan.files[0];
        assert_eq!(file.status, Status::Invalid);
        assert!(file.overwritable, "{file:?}");

        // Not chosen: refused as before.
        let refused = ApplyChoice {
            overwrite: Vec::new(),
            ..choose_all(&plan)
        };
        let head = harness.head();
        let done = harness
            .run({
                let (input, head) = (input.clone(), head.clone());
                move |repo| apply_input(repo, &input, &PathMapping::default(), &refused, &head)
            })
            .unwrap();
        assert_eq!(done.status, ApplyStatus::Failure);
        assert_eq!(fs::read(harness.path().join("c.bin")).unwrap(), [0, 3]);

        let chosen = ApplyChoice {
            overwrite: vec!["c.bin".to_string()],
            ..choose_all(&plan)
        };
        let done = harness
            .run(move |repo| apply_input(repo, &input, &PathMapping::default(), &chosen, &head))
            .unwrap();
        assert_eq!(done.status, ApplyStatus::Success, "{done:?}");
        assert_eq!(fs::read(harness.path().join("c.bin")).unwrap(), [0, 9]);
        let kept = snapshots::list(harness.data.path())
            .into_iter()
            .any(|copy| {
                snapshots::stored(harness.data.path(), &copy.id, "c.bin", false)
                    .is_some_and(|kept| fs::read(kept).unwrap() == [0, 3])
            });
        assert!(kept);
    }

    // Finding 7: every way to a list that is gone says the same.
    #[test]
    fn a_list_that_is_gone_is_named_the_same_way_everywhere() {
        let harness = RepoHarness::new();
        let data = harness.data.path();
        let gone = "\"Gone\" is no longer in the shelf";

        assert_eq!(source(data, "Gone", None).err().unwrap(), gone);
        assert_eq!(
            shelf::load_patch(data, "Gone").unwrap_err().to_string(),
            gone
        );
        let error = unshelve::recycle_applied(data, "Gone", &[], at(1)).unwrap_err();
        assert_eq!(error, crate::git::GitError::Io(gone.to_string()));
        let error = harness
            .run(|repo| unshelve::unshelve(repo, "Gone", None, at(1)))
            .unwrap_err();
        assert_eq!(error, crate::git::GitError::Io(gone.to_string()));
    }

    use crate::patch::WriteOptions;
    use crate::patch_apply::{apply_input, plan_input, ApplyChoice, ApplyStatus, Status};
    use crate::shelf::unshelve::{HeadChanged, PutBackMoment};
    use crate::shelf::{unshelve, BinaryChange};
    use crate::test_support::RepoHarness;
    use std::os::unix::fs::symlink;
    use std::time::Duration;
    use tempfile::TempDir;

    const LINES: &str = "1\n2\n3\n4\n5\n6\n7\n8\n9\n";

    fn at(ms: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(ms)
    }

    // A list as Shelve makes it: each text file is HEAD against `after` (`None`: deleted), with
    // the base stored; each binary file is copied from the disk, which is then put back to HEAD.
    fn shelve(
        harness: &RepoHarness,
        description: &str,
        texts: &[(&str, Option<&str>)],
        binaries: &[(&str, Option<&[u8]>)],
    ) -> ShelvedList {
        let repo = harness.path();
        let sources: Vec<PatchSource> = texts
            .iter()
            .map(|(path, after)| PatchSource {
                before: local(repo, path).map(|bytes| (path.to_string(), bytes)),
                after: after.map(|text| (path.to_string(), text.as_bytes().to_vec())),
                before_rev: None,
                binary: false,
            })
            .collect();
        let options = WriteOptions {
            include_base: true,
            ..WriteOptions::default()
        };
        let patch = patch::build(repo, &sources, &options).unwrap();
        let mut changes = Vec::new();
        for (path, bytes) in binaries {
            let existed = repo.join(path).exists();
            match bytes {
                Some(bytes) => fs::write(repo.join(path), bytes).unwrap(),
                None => fs::remove_file(repo.join(path)).unwrap(),
            }
            changes.push(BinaryChange {
                before: existed.then(|| path.to_string()),
                after: bytes.map(|_| path.to_string()),
            });
        }
        let list = shelf::create(
            harness.data.path(),
            repo,
            description,
            &patch,
            &changes,
            false,
        )
        .unwrap();
        for (path, _) in binaries {
            if harness.git(&["ls-files", "--", path]).trim().is_empty() {
                let _ = fs::remove_file(repo.join(path));
            } else {
                harness.git(&["checkout", "--", path]);
            }
        }
        list
    }

    fn shelf_input(name: &str) -> PatchInput {
        PatchInput::Shelf {
            name: name.to_string(),
            files: None,
        }
    }

    fn choose_all(plan: &crate::patch_apply::ApplyPlan) -> ApplyChoice {
        ApplyChoice {
            selected: plan.files.iter().map(|file| file.path.clone()).collect(),
            overwrite: Vec::new(),
            expected: plan
                .files
                .iter()
                .flat_map(|file| file.hashes.clone())
                .collect(),
        }
    }

    fn unshelve_through_the_dialog(
        harness: &RepoHarness,
        name: &str,
    ) -> crate::patch_apply::ApplyResult {
        let input = shelf_input(name);
        let plan = plan_input(
            harness.path(),
            harness.data.path(),
            &input,
            &PathMapping::default(),
        )
        .unwrap();
        let choice = choose_all(&plan);
        let head = harness.head();
        harness
            .run(move |repo| apply_input(repo, &input, &PathMapping::default(), &choice, &head))
            .unwrap()
    }

    fn the_list(harness: &RepoHarness, name: &str) -> ShelvedList {
        find(harness.data.path(), name).unwrap()
    }

    #[test]
    fn unshelve_uses_the_stored_base_for_conflicts() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", LINES.as_bytes(), "first");
        harness.repo.commit("b.txt", LINES.as_bytes(), "second");
        let list = shelve(
            &harness,
            "Work",
            &[
                ("a.txt", Some("1\ntwo\n3\n4\n5\n6\n7\n8\n9\n")),
                ("b.txt", Some("1\ntwo\n3\n4\n5\n6\n7\n8\n9\n")),
            ],
            &[],
        );
        // What an Update brings meanwhile: a.txt changes the same line, b.txt a line of the hunk's
        // context, so that the hunk no longer fits as it was written.
        harness
            .repo
            .commit("a.txt", b"1\nTWO\n3\n4\n5\n6\n7\n8\n9\n", "upstream a");
        harness
            .repo
            .commit("b.txt", b"1\n2\n3\n4\nFIVE\n6\n7\n8\n9\n", "upstream b");

        let applied = unshelve_through_the_dialog(&harness, &list.name);

        // b.txt merges with the stored base; a.txt needs the merge window, with that base.
        assert_eq!(applied.conflicts.len(), 1, "{applied:?}");
        let conflict = &applied.conflicts[0];
        assert_eq!(conflict.path, "a.txt");
        assert_eq!(conflict.base, LINES);
        assert_eq!(conflict.local, "1\nTWO\n3\n4\n5\n6\n7\n8\n9\n");
        assert_eq!(conflict.patched, "1\ntwo\n3\n4\n5\n6\n7\n8\n9\n");
        assert_eq!(harness.read("a.txt"), "1\nTWO\n3\n4\n5\n6\n7\n8\n9\n");
        assert!(applied.failed.is_empty(), "{applied:?}");
        assert!(applied.snapshot.is_some());
        assert_eq!(harness.read("b.txt"), "1\ntwo\n3\n4\nFIVE\n6\n7\n8\n9\n");
        // Without the dialog's Remove, the list keeps everything.
        assert_eq!(the_list(&harness, &list.name), list);
    }

    #[test]
    fn devin_writing_during_unshelve_is_not_overwritten() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", LINES.as_bytes(), "first");
        harness.repo.commit("c.bin", &[0, 1, 2], "binary");
        let list = shelve(
            &harness,
            "Work",
            &[("a.txt", Some("one\n2\n3\n4\n5\n6\n7\n8\n9\n"))],
            &[("c.bin", Some(&[0, 9, 9]))],
        );
        let input = shelf_input(&list.name);
        let plan = plan_input(
            harness.path(),
            harness.data.path(),
            &input,
            &PathMapping::default(),
        )
        .unwrap();
        assert_eq!(plan.files.len(), 2);
        assert!(plan.files[1].binary);
        assert_eq!(plan.files[1].status, Status::Applicable);
        let choice = choose_all(&plan);

        // Devin writes both files after the dialog read them.
        harness.write("a.txt", "devin\n");
        fs::write(harness.path().join("c.bin"), [7, 0, 7]).unwrap();
        let head = harness.head();
        let applied = harness
            .run(move |repo| apply_input(repo, &input, &PathMapping::default(), &choice, &head))
            .unwrap();

        assert_eq!(applied.failed, ["a.txt", "c.bin"]);
        assert_eq!(applied.status, ApplyStatus::Failure);
        assert_eq!(harness.read("a.txt"), "devin\n");
        assert_eq!(fs::read(harness.path().join("c.bin")).unwrap(), [7, 0, 7]);
        // The copy was taken first: what Devin wrote can always come back.
        let saved = snapshots::list(harness.data.path());
        assert!(!saved.is_empty());
        assert_eq!(the_list(&harness, &list.name), list);
    }

    #[test]
    fn a_shelved_list_plans_its_binaries_and_only_the_chosen_files() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", LINES.as_bytes(), "first");
        harness.repo.commit("b.txt", LINES.as_bytes(), "second");
        let list = shelve(
            &harness,
            "Work",
            &[("a.txt", Some("x\n")), ("b.txt", None)],
            &[("n.bin", Some(&[1, 0]))],
        );
        let input = shelf_input(&list.name);
        let plan = plan_input(
            harness.path(),
            harness.data.path(),
            &input,
            &PathMapping::default(),
        )
        .unwrap();
        let paths: Vec<_> = plan.files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(paths, ["a.txt", "b.txt", "n.bin"]);
        assert_eq!(
            plan.files
                .iter()
                .map(|file| file.shelf.clone())
                .collect::<Vec<_>>(),
            [
                Some(ShelfFile {
                    before: Some("a.txt".into()),
                    after: Some("a.txt".into()),
                    binary: false
                }),
                Some(ShelfFile {
                    before: Some("b.txt".into()),
                    after: None,
                    binary: false
                }),
                Some(ShelfFile {
                    before: None,
                    after: Some("n.bin".into()),
                    binary: true
                }),
            ]
        );
        // A new binary file whose place is taken cannot be included.
        fs::write(harness.path().join("n.bin"), [5]).unwrap();
        let again = plan_input(
            harness.path(),
            harness.data.path(),
            &input,
            &PathMapping::default(),
        )
        .unwrap();
        assert_eq!(again.files[2].status, Status::Invalid);
        assert_eq!(
            again.files[2].error.as_deref(),
            Some("\"n.bin\" changed on disk while it was shelved: it was not unshelved")
        );
        fs::remove_file(harness.path().join("n.bin")).unwrap();

        let only = PatchInput::Shelf {
            name: list.name.clone(),
            files: Some(vec![plan.files[2].shelf.clone().unwrap()]),
        };
        let plan = plan_input(
            harness.path(),
            harness.data.path(),
            &only,
            &PathMapping::default(),
        )
        .unwrap();
        assert_eq!(plan.files.len(), 1);
        assert_eq!(plan.files[0].index, 0);
        let choice = choose_all(&plan);
        let head = harness.head();
        let applied = harness
            .run(move |repo| apply_input(repo, &only, &PathMapping::default(), &choice, &head))
            .unwrap();
        assert_eq!(applied.status, ApplyStatus::Success, "{applied:?}");
        assert_eq!(fs::read(harness.path().join("n.bin")).unwrap(), [1, 0]);
        assert_eq!(harness.read("a.txt"), LINES);
    }

    #[test]
    fn a_binary_changed_in_head_since_shelving_stays_in_the_list() {
        let harness = RepoHarness::new();
        harness.repo.commit("c.bin", &[0, 1], "first");
        harness.repo.commit("d.bin", &[0, 2], "second");
        let list = shelve(
            &harness,
            "Work",
            &[],
            &[("c.bin", Some(&[0, 9])), ("d.bin", None)],
        );
        // An Update changes both: the disk is the new HEAD, untouched.
        harness.repo.commit("c.bin", &[0, 3], "upstream c");
        harness.repo.commit("d.bin", &[0, 4], "upstream d");

        let done = harness
            .run({
                let name = list.name.clone();
                move |repo| unshelve::unshelve(repo, &name, None, at(5))
            })
            .unwrap();
        assert_eq!(
            done.binaries_failed,
            [
                "\"c.bin\" changed in HEAD since it was shelved: it was not unshelved",
                "\"d.bin\" changed in HEAD since it was shelved: it was not unshelved",
            ]
        );
        assert!(done.remaining);
        assert_eq!(fs::read(harness.path().join("c.bin")).unwrap(), [0, 3]);
        assert_eq!(fs::read(harness.path().join("d.bin")).unwrap(), [0, 4]);
        assert_eq!(the_list(&harness, &list.name), list);

        // The dialog shows them as not applicable, for the same reason.
        let plan = plan_input(
            harness.path(),
            harness.data.path(),
            &shelf_input(&list.name),
            &PathMapping::default(),
        )
        .unwrap();
        assert!(plan.files.iter().all(|file| file.status == Status::Invalid));

        // HEAD as it was when shelved: they go back.
        harness.git(&["reset", "-q", "--hard", "HEAD~2"]);
        let done = harness
            .run({
                let name = list.name.clone();
                move |repo| unshelve::unshelve(repo, &name, None, at(6))
            })
            .unwrap();
        assert!(done.binaries_failed.is_empty(), "{done:?}");
        assert_eq!(fs::read(harness.path().join("c.bin")).unwrap(), [0, 9]);
        assert!(!harness.path().join("d.bin").exists());
    }

    #[test]
    fn remove_applied_recycles_or_splits_as_241() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", LINES.as_bytes(), "first");
        harness.repo.commit("c.txt", LINES.as_bytes(), "second");
        let data = harness.data.path();
        let list = shelve(
            &harness,
            "Work",
            &[("a.txt", Some("a\n")), ("c.txt", Some("c\n"))],
            &[("x.bin", Some(&[1, 0]))],
        );
        let a = ShelfFile {
            before: Some("a.txt".into()),
            after: Some("a.txt".into()),
            binary: false,
        };
        let x = ShelfFile {
            before: None,
            after: Some("x.bin".into()),
            binary: true,
        };

        // Nothing applied: nothing changes.
        unshelve::recycle_applied(data, &list.name, &[], at(10)).unwrap();
        assert_eq!(shelf::list(data), std::slice::from_ref(&list));

        // A part: a recycled copy takes it, dated now; the list keeps the rest and its date.
        unshelve::recycle_applied(data, &list.name, &[a.clone(), x.clone()], at(20)).unwrap();
        let lists = entries(data);
        assert_eq!(lists.len(), 2);
        let copy = lists
            .iter()
            .find(|entry| entry.list.name != list.name)
            .unwrap();
        assert!(copy.list.recycled);
        assert_eq!(copy.list.date_ms, 20);
        assert_eq!(copy.list.description, "Work");
        let files: Vec<_> = copy
            .changes
            .iter()
            .map(|change| change.file.clone())
            .collect();
        assert_eq!(files, [a.clone(), x.clone()]);
        let kept = lists
            .iter()
            .find(|entry| entry.list.name == list.name)
            .unwrap();
        assert!(!kept.list.recycled);
        assert_eq!(kept.list.date_ms, list.date_ms);
        let paths: Vec<_> = kept
            .changes
            .iter()
            .map(|change| change.path.as_str())
            .collect();
        assert_eq!(paths, ["c.txt"]);

        // The rest: the list itself is recycled.
        let c = kept.changes[0].file.clone();
        unshelve::recycle_applied(data, &list.name, &[c], at(30)).unwrap();
        let recycled = the_list(&harness, &list.name);
        assert!(recycled.recycled);
        assert_eq!(recycled.date_ms, 30);
        // Already recycled: a file it does not hold changes nothing.
        unshelve::recycle_applied(data, &list.name, &[a], at(40)).unwrap();
        assert_eq!(the_list(&harness, &list.name), recycled);
    }

    // `saveRemainingPatchesIfNeeded`: nothing left recycles the list again, with the new date, even
    // when it was recycled already; a part of a recycled list changes nothing.
    #[test]
    fn a_recycled_list_applied_again_whole_is_recycled_with_the_new_date() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", LINES.as_bytes(), "first");
        harness.repo.commit("c.txt", LINES.as_bytes(), "second");
        let data = harness.data.path();
        let list = shelve(
            &harness,
            "Work",
            &[("a.txt", Some("a\n")), ("c.txt", Some("c\n"))],
            &[],
        );
        let files: Vec<ShelfFile> = entries(data)[0]
            .changes
            .iter()
            .map(|change| change.file.clone())
            .collect();
        unshelve::recycle_applied(data, &list.name, &files, at(10)).unwrap();
        assert_eq!(the_list(&harness, &list.name).date_ms, 10);

        unshelve::recycle_applied(data, &list.name, &files[..1], at(20)).unwrap();
        assert_eq!(the_list(&harness, &list.name).date_ms, 10);
        assert_eq!(entries(data).len(), 1);

        unshelve::recycle_applied(data, &list.name, &files, at(30)).unwrap();
        let again = the_list(&harness, &list.name);
        assert!(again.recycled);
        assert_eq!(again.date_ms, 30);
    }

    // As `patch_apply`: the copy is taken first, then compared with what the plan read, so what
    // Devin wrote meanwhile is in the copy even when the binary is not put back.
    #[test]
    fn put_back_takes_the_copy_before_it_checks() {
        let harness = RepoHarness::new();
        harness.repo.commit("c.bin", &[0, 1, 2], "binary");
        let list = shelve(&harness, "Work", &[], &[("c.bin", Some(&[0, 9, 9]))]);
        let input = shelf_input(&list.name);
        let plan = plan_input(
            harness.path(),
            harness.data.path(),
            &input,
            &PathMapping::default(),
        )
        .unwrap();
        let choice = choose_all(&plan);
        // Devin's write keeps git's view of the file: only the plan's hash can tell.
        harness.git(&["update-index", "--assume-unchanged", "c.bin"]);
        fs::write(harness.path().join("c.bin"), [7, 0, 7]).unwrap();
        let head = harness.head();
        let applied = harness
            .run(move |repo| apply_input(repo, &input, &PathMapping::default(), &choice, &head))
            .unwrap();

        assert_eq!(applied.failed, ["c.bin"]);
        assert_eq!(fs::read(harness.path().join("c.bin")).unwrap(), [7, 0, 7]);
        let saved = snapshots::list(harness.data.path());
        let copy = saved
            .iter()
            .find(|snapshot| snapshot.entries.iter().any(|entry| entry.path == "c.bin"))
            .expect("a copy of c.bin");
        let stored = snapshots::stored_files(harness.data.path(), &copy.id).unwrap();
        assert_eq!(fs::read(&stored["c.bin"]).unwrap(), [7, 0, 7]);
    }

    // A moved binary: its source is removed only if it is still what the copy holds. Written to
    // during the move, it is left, and said.
    #[test]
    fn a_binary_move_leaves_a_source_written_during_the_move() {
        let harness = RepoHarness::new();
        harness.repo.commit("old.bin", &[0, 1], "binary");
        let repo = harness.path();
        fs::rename(repo.join("old.bin"), repo.join("new.bin")).unwrap();
        fs::write(repo.join("new.bin"), [0, 2]).unwrap();
        let change = BinaryChange {
            before: Some("old.bin".into()),
            after: Some("new.bin".into()),
        };
        let list = shelf::create(
            harness.data.path(),
            repo,
            "Move",
            &Patch::default(),
            &[change],
            false,
        )
        .unwrap();
        fs::remove_file(repo.join("new.bin")).unwrap();
        harness.git(&["checkout", "--", "old.bin"]);
        let binary = list.binaries[0].clone();
        let base_ids = shelf::base_ids(&list);

        let root = repo.to_path_buf();
        let moved = harness.run(move |repo| {
            unshelve::put_back_watching(
                repo,
                &binary,
                &base_ids,
                None,
                HeadChanged::Refuse,
                &mut |moment| {
                    if moment == PutBackMoment::Remove {
                        fs::write(root.join("old.bin"), [9, 9]).unwrap();
                    }
                },
            )
        });
        assert_eq!(
            moved,
            Err("\"old.bin\" changed while it was unshelved: it was left as it is".to_string())
        );
        assert_eq!(fs::read(harness.path().join("old.bin")).unwrap(), [9, 9]);
        assert_eq!(fs::read(harness.path().join("new.bin")).unwrap(), [0, 2]);
    }

    // P25: the target of a binary is also checked right before it is written: what another
    // program wrote there after the copy stays, and so does the source of a move.
    #[test]
    fn a_binary_target_written_after_the_copy_is_left() {
        let harness = RepoHarness::new();
        harness.repo.commit("c.bin", &[0, 1], "binary");
        harness.repo.commit("old.bin", &[0, 3], "moved");
        let repo = harness.path();
        fs::write(repo.join("c.bin"), [0, 2]).unwrap();
        fs::rename(repo.join("old.bin"), repo.join("new.bin")).unwrap();
        let changes = [
            BinaryChange {
                before: Some("c.bin".into()),
                after: Some("c.bin".into()),
            },
            BinaryChange {
                before: Some("old.bin".into()),
                after: Some("new.bin".into()),
            },
        ];
        let list = shelf::create(
            harness.data.path(),
            repo,
            "Work",
            &Patch::default(),
            &changes,
            false,
        )
        .unwrap();
        harness.git(&["checkout", "--", "c.bin", "old.bin"]);
        fs::remove_file(repo.join("new.bin")).unwrap();
        let base_ids = shelf::base_ids(&list);

        for (binary, target) in list.binaries.iter().zip(["c.bin", "new.bin"]) {
            let (binary, base_ids) = (binary.clone(), base_ids.clone());
            let full = repo.join(target);
            let put = harness.run(move |repo| {
                unshelve::put_back_watching(
                    repo,
                    &binary,
                    &base_ids,
                    None,
                    HeadChanged::Refuse,
                    &mut |moment| {
                        if moment == PutBackMoment::Write {
                            fs::write(&full, [7, 7]).unwrap();
                        }
                    },
                )
            });
            assert_eq!(
                put,
                Err(format!(
                    "\"{target}\" changed while it was unshelved: it was left as it is"
                ))
            );
            assert_eq!(fs::read(repo.join(target)).unwrap(), [7, 7]);
        }
        assert_eq!(fs::read(repo.join("old.bin")).unwrap(), [0, 3]);
    }

    // A list that cannot be deleted does not lose the Undo of those that were.
    #[test]
    fn a_failed_delete_still_gives_back_what_was_deleted() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", LINES.as_bytes(), "first");
        let data = harness.data.path();
        let one = shelve(&harness, "One", &[("a.txt", Some("a\n"))], &[]);
        let two = shelve(&harness, "Two", &[("a.txt", Some("b\n"))], &[]);
        fs::write(&two.patch, "--- a/x\n+++ b/x\n@@ -a +1 @@\n-x\n").unwrap();

        let deleted = delete(
            data,
            &[one.name.clone(), "Gone".to_string()],
            &[ListFiles {
                name: two.name.clone(),
                files: vec![ShelfFile {
                    before: Some("a.txt".into()),
                    after: Some("a.txt".into()),
                    binary: false,
                }],
            }],
            at(50),
        )
        .unwrap();
        assert_eq!(
            deleted.restorable,
            [Restorable {
                name: one.name.clone(),
                date_ms: one.date_ms
            }]
        );
        assert_eq!(deleted.errors.len(), 2, "{deleted:?}");
        // The message counts what was really deleted.
        assert_eq!(deleted.lists, 1);
        assert_eq!(deleted.first.as_deref(), Some("One"));
        assert_eq!(deleted.files, 0);
        assert!(the_list(&harness, &one.name).deleted);
        assert!(!the_list(&harness, &two.name).deleted);
    }

    #[test]
    fn import_errors_are_the_users_texts() {
        let harness = RepoHarness::new();
        let outside = TempDir::new().unwrap();
        let file = outside.path().join("bad.patch");
        fs::write(&file, "--- a/x\nsomething\n").unwrap();
        let imported = import_patches(harness.data.path(), std::slice::from_ref(&file));
        assert_eq!(
            imported.errors,
            [format!(
                "{}: Second file name expected (line 2)",
                file.display()
            )]
        );
        assert_eq!(
            patch_error_text(patch::PatchError::BinaryAsText { path: "x".into() }),
            "\"x\" is not text"
        );
    }

    // `ImportToShelfExecutor` takes the text patches only.
    #[test]
    fn import_to_shelf_keeps_the_text_patches_only() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"a\n", "first");
        harness.repo.commit("c.bin", &[0, 1, 2], "binary");
        fs::write(harness.path().join("a.txt"), "b\n").unwrap();
        fs::write(harness.path().join("c.bin"), [0, 3]).unwrap();
        let text = harness.git(&["diff", "--binary"]);
        harness.git(&["checkout", "--", "."]);
        let file = harness.path().join("both.patch");
        fs::write(&file, text).unwrap();
        let source = PatchInput::File {
            path: file.to_string_lossy().into_owned(),
        };
        let list = import_to_shelf(
            harness.data.path(),
            harness.path(),
            &source,
            &PathMapping::default(),
            &["a.txt".to_string(), "c.bin".to_string()],
        )
        .unwrap();
        let stored = shelf::load_patch(harness.data.path(), &list.name).unwrap();
        assert_eq!(stored.files.len(), 1);
        assert!(matches!(stored.files[0], FilePatch::Text(_)));
    }

    // View saved changes… after an Update stopped: the system list is unshelved, its conflict is
    // merged in the window, and the merged file leaves the shelf.
    #[test]
    fn a_system_list_with_a_conflict_is_merged_and_recycled() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", LINES.as_bytes(), "first");
        harness.repo.commit("b.txt", LINES.as_bytes(), "second");
        let repo = harness.path();
        let sources: Vec<PatchSource> = ["a.txt", "b.txt"]
            .iter()
            .map(|path| PatchSource {
                before: local(repo, path).map(|bytes| (path.to_string(), bytes)),
                after: Some((path.to_string(), b"1\ntwo\n3\n4\n5\n6\n7\n8\n9\n".to_vec())),
                before_rev: None,
                binary: false,
            })
            .collect();
        let options = WriteOptions {
            include_base: true,
            ..WriteOptions::default()
        };
        let patch = patch::build(repo, &sources, &options).unwrap();
        let description = "Uncommitted changes before Update at 11/8/26 [Changes]";
        let list =
            shelf::create(harness.data.path(), repo, description, &patch, &[], true).unwrap();
        harness
            .repo
            .commit("a.txt", b"1\nTWO\n3\n4\n5\n6\n7\n8\n9\n", "upstream");

        let applied = unshelve_through_the_dialog(&harness, &list.name);
        assert_eq!(applied.conflicts.len(), 1);
        let conflict = applied.conflicts[0].clone();
        let merged = patch_apply::resolve_conflict(
            repo,
            &conflict.path,
            "1\nTWO two\n3\n4\n5\n6\n7\n8\n9\n",
            &conflict.local_hash,
            None,
            None,
        )
        .unwrap();
        assert!(merged.messages.is_empty());
        assert_eq!(harness.read("a.txt"), "1\nTWO two\n3\n4\n5\n6\n7\n8\n9\n");

        // Both files went through: b.txt applied, a.txt merged. The system list becomes deleted.
        let files: Vec<ShelfFile> = entries(harness.data.path())[0]
            .changes
            .iter()
            .map(|change| change.file.clone())
            .collect();
        unshelve::recycle_applied(harness.data.path(), &list.name, &files, at(70)).unwrap();
        let done = the_list(&harness, &list.name);
        assert!(done.recycled);
        assert!(done.deleted);
    }

    #[test]
    fn delete_then_undo_restores_the_list() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", LINES.as_bytes(), "first");
        harness.repo.commit("c.txt", LINES.as_bytes(), "second");
        let data = harness.data.path();
        let one = shelve(
            &harness,
            "One",
            &[("a.txt", Some("a\n")), ("c.txt", Some("c\n"))],
            &[],
        );
        let two = shelve(&harness, "Two", &[("a.txt", Some("b\n"))], &[]);
        let a = ShelfFile {
            before: Some("a.txt".into()),
            after: Some("a.txt".into()),
            binary: false,
        };

        let deleted = delete(
            data,
            std::slice::from_ref(&two.name),
            &[
                ListFiles {
                    name: one.name.clone(),
                    files: vec![a.clone()],
                },
                // A file of a list deleted whole counts with its list.
                ListFiles {
                    name: two.name.clone(),
                    files: vec![a.clone()],
                },
            ],
            at(50),
        )
        .unwrap();
        assert_eq!(deleted.lists, 1);
        assert_eq!(deleted.files, 1);
        assert_eq!(deleted.first.as_deref(), Some("Two"));
        assert_eq!(deleted.restorable.len(), 2);
        assert!(the_list(&harness, &two.name).deleted);
        let kept = entries(data)
            .into_iter()
            .find(|entry| entry.list.name == one.name)
            .unwrap();
        assert!(!kept.list.deleted);
        assert_eq!(kept.changes.len(), 1);
        let copy = &deleted.restorable[1];
        assert!(the_list(&harness, &copy.name).deleted);
        assert_eq!(copy.date_ms, one.date_ms);

        assert_eq!(undo_delete(data, &deleted.restorable).unwrap(), 0);
        let back = the_list(&harness, &two.name);
        assert!(!back.deleted);
        assert_eq!(back.date_ms, two.date_ms);
        let copy = the_list(&harness, &copy.name);
        assert!(!copy.deleted);
        assert_eq!(copy.date_ms, one.date_ms);

        // Deleting a deleted list erases it: nothing comes back, and Undo says so.
        let first = delete(data, std::slice::from_ref(&two.name), &[], at(60)).unwrap();
        let second = delete(data, std::slice::from_ref(&two.name), &[], at(70)).unwrap();
        assert!(second.restorable.is_empty());
        assert!(find(data, &two.name).is_err());
        assert_eq!(undo_delete(data, &first.restorable).unwrap(), 1);

        // Restore brings a deleted list back, dated now.
        let three = shelve(&harness, "Three", &[("c.txt", Some("3\n"))], &[]);
        delete(data, std::slice::from_ref(&three.name), &[], at(80)).unwrap();
        restore(data, std::slice::from_ref(&three.name), at(90)).unwrap();
        let restored = the_list(&harness, &three.name);
        assert!(!restored.deleted);
        assert_eq!(restored.date_ms, 90);
    }

    // Part 3's N3: a list the weekly cleanup erased, moved by hand from `shelf-erased/` into
    // `shelf-restore/`, comes back whole (its binary copy too), no longer deleted and dated now,
    // so the next cleanup keeps it. One whose name is taken again stays where it was put.
    #[test]
    fn a_list_restored_from_erased_survives_the_next_cleanup() {
        const DAY: u64 = 24 * 60 * 60 * 1000;
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", LINES.as_bytes(), "first");
        let data = harness.data.path();
        let list = shelve(
            &harness,
            "Old work",
            &[("a.txt", Some("a\n"))],
            &[("x.png", Some(&[0, 1, 2]))],
        );
        delete(data, std::slice::from_ref(&list.name), &[], at(DAY)).unwrap();
        assert_eq!(shelf::cleanup(data, at(9 * DAY)).unwrap(), 1);
        assert!(find(data, &list.name).is_err());
        let erased = data.join(format!("shelf-erased/{}-{}", 9 * DAY, list.name));
        assert!(erased.is_dir());
        fs::create_dir_all(data.join("shelf-restore")).unwrap();
        let request = data.join("shelf-restore").join(erased.file_name().unwrap());
        fs::rename(&erased, &request).unwrap();

        let restored = restore_requested(data, at(10 * DAY));

        assert_eq!(restored.lists.len(), 1, "{restored:?}");
        assert!(restored.errors.is_empty(), "{restored:?}");
        assert!(!request.exists());
        let back = the_list(&harness, &list.name);
        assert!(!back.deleted);
        assert_eq!(back.date_ms, 10 * DAY);
        assert_eq!(back.binaries.len(), 1);
        assert_eq!(
            fs::read(back.binaries[0].shelved.as_ref().unwrap()).unwrap(),
            [0, 1, 2]
        );
        assert_eq!(shelf::cleanup(data, at(40 * DAY)).unwrap(), 0);
        assert!(!the_list(&harness, &list.name).deleted);

        // The name is taken: the request stays, and says why.
        let other = shelve(&harness, "Other", &[("a.txt", Some("b\n"))], &[]);
        delete(data, std::slice::from_ref(&other.name), &[], at(41 * DAY)).unwrap();
        delete(data, std::slice::from_ref(&other.name), &[], at(41 * DAY)).unwrap();
        let erased = data.join(format!("shelf-erased/{}-{}", 41 * DAY, other.name));
        let request = data.join("shelf-restore").join(erased.file_name().unwrap());
        fs::rename(&erased, &request).unwrap();
        shelve(&harness, "Other", &[("a.txt", Some("c\n"))], &[]);

        let refused = restore_requested(data, at(42 * DAY));

        assert!(refused.lists.is_empty());
        assert_eq!(refused.errors.len(), 1, "{refused:?}");
        assert!(request.join(format!("{}.xml", other.name)).is_file());
        assert!(request.join(&other.name).is_dir());
    }

    #[test]
    fn rename_changes_the_description_only() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", LINES.as_bytes(), "first");
        let list = shelve(&harness, "Work", &[("a.txt", Some("a\n"))], &[]);
        let renamed = shelf::rename(harness.data.path(), &list.name, "Other work").unwrap();
        assert_eq!(renamed.description, "Other work");
        assert_eq!(renamed.name, list.name);
        assert_eq!(renamed.patch, list.patch);
        assert_eq!(renamed.date_ms, list.date_ms);
    }

    #[test]
    fn import_patches_finds_files_and_asks() {
        let harness = RepoHarness::new();
        let data = harness.data.path();
        let outside = TempDir::new().unwrap();
        let root = outside.path();
        let patch = "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-a\n+b\n";
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("my_fix.patch"), patch).unwrap();
        fs::write(root.join("sub/other.DIFF"), patch).unwrap();
        fs::write(root.join("sub/notes.txt"), patch).unwrap();
        fs::write(root.join("empty.patch"), "").unwrap();
        fs::write(
            root.join("broken.patch"),
            "--- a/x\n+++ b/x\n@@ -a +1 @@\n-x\n",
        )
        .unwrap();
        let elsewhere = TempDir::new().unwrap();
        fs::write(elsewhere.path().join("linked.patch"), patch).unwrap();
        symlink(elsewhere.path(), root.join("link")).unwrap();

        let found = find_patches(&[root.to_path_buf()]);
        let names: Vec<_> = found
            .iter()
            .map(|path| {
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(
            names,
            [
                "broken.patch",
                "empty.patch",
                "my_fix.patch",
                "sub/other.DIFF"
            ]
        );
        // A chosen file that is not a patch is not one of them.
        assert!(find_patches(&[root.join("sub/notes.txt")]).is_empty());

        let mtime = fs::metadata(root.join("my_fix.patch"))
            .unwrap()
            .modified()
            .unwrap();
        let imported = import_patches(data, &found);
        assert_eq!(imported.errors.len(), 1, "{imported:?}");
        assert!(imported.errors[0].contains("broken.patch"));
        let descriptions: Vec<_> = imported
            .lists
            .iter()
            .map(|list| list.description.as_str())
            .collect();
        assert_eq!(descriptions, ["my fix", "other"]);
        assert_eq!(imported.lists[0].date_ms, millis(mtime));
        assert_eq!(fs::read_to_string(&imported.lists[0].patch).unwrap(), patch);
        assert_eq!(shelf::list(data).len(), 2);
        // The chosen files are only read.
        assert_eq!(
            fs::read_to_string(root.join("my_fix.patch")).unwrap(),
            patch
        );
    }

    #[test]
    fn import_to_shelf_keeps_the_mapped_paths_and_the_bases() {
        let harness = RepoHarness::new();
        harness.repo.commit("src/a.txt", b"a\n", "first");
        let file = harness.path().join("fix.patch");
        fs::write(
            &file,
            "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-a\n+b\n--- a/z.txt\n+++ b/z.txt\n@@ -1 +1 @@\n-z\n+y\n",
        )
        .unwrap();
        let source = PatchInput::File {
            path: file.to_string_lossy().into_owned(),
        };
        let mapping: PathMapping =
            serde_json::from_str(r#"{"files":[{"strip":0,"base":"src"}]}"#).unwrap();
        let list = import_to_shelf(
            harness.data.path(),
            harness.path(),
            &source,
            &mapping,
            &["src/a.txt".to_string()],
        )
        .unwrap();
        assert_eq!(list.description, "fix.patch");
        let stored = shelf::load_patch(harness.data.path(), &list.name).unwrap();
        assert_eq!(stored.files.len(), 1);
        assert_eq!(
            ShelfFile::of_patch(&stored.files[0]).after.as_deref(),
            Some("src/a.txt")
        );
    }

    #[test]
    fn show_diff_is_the_base_against_the_shelved_version() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", LINES.as_bytes(), "first");
        let list = shelve(
            &harness,
            "Work",
            &[("a.txt", Some("one\n2\n3\n4\n5\n6\n7\n8\n9\n"))],
            &[],
        );
        let file = entries(harness.data.path())[0].changes[0].file.clone();
        harness.write("a.txt", "local\n");
        let shown = sides(
            harness.data.path(),
            harness.path(),
            &list.name,
            &file,
            false,
        )
        .unwrap();
        assert_eq!(
            shown.left,
            Side::Text {
                content: LINES.to_string()
            }
        );
        assert_eq!(
            shown.right,
            Side::Text {
                content: "one\n2\n3\n4\n5\n6\n7\n8\n9\n".to_string()
            }
        );
        assert_eq!(shown.titles, ["Base version", "Shelved version"]);
        let local = sides(harness.data.path(), harness.path(), &list.name, &file, true).unwrap();
        assert_eq!(
            local.left,
            Side::Text {
                content: "local\n".to_string()
            }
        );
        assert_eq!(local.titles, ["Current version", "Shelved version"]);
    }

    #[test]
    fn a_list_whose_patch_cannot_be_read_shows_why() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", LINES.as_bytes(), "first");
        let list = shelve(&harness, "Work", &[("a.txt", Some("a\n"))], &[]);
        fs::write(&list.patch, "--- a/x\n+++ b/x\n@@ -a +1 @@\n-x\n").unwrap();
        let shown = entries(harness.data.path());
        assert!(shown[0].changes.is_empty());
        assert!(shown[0]
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("Cannot load patch file: ")));
    }
}
