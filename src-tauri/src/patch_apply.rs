// Applying a patch, with a recoverable copy first. Read at the tag idea/241.14494.240.
//
// - Group flow: `PatchApplier.executePatchGroup`, `nonWriteActionPreCheck`, `executeWritable`,
//   `applyList` and `applyAdditionalPatchData`; `PathsVerifier` (the checks of added, deleted,
//   moved and modified files, `doMoveIfNeeded`); `TriggerAdditionOrDeletion`.
// - One file: `ApplyPatchAction.applyContent`, `ApplyTextFilePatch`, `ApplyFilePatchBase`,
//   `PlainSimplePatchApplier`, `ApplyPatchForBaseRevisionTexts` and
//   `DefaultPatchBaseVersionProvider` (the `(revision h)` base).
// - Paths: `AbstractFilePatchInProgress` (`StripCapablePath`, `PatchStrippable`).
// - Statuses: `ApplyPatchStatus`.
//
// Decision 4 of the plan replaces IntelliJ's fuzzy `GenericPatchApplier`. A text file is tried:
// 1. each hunk exactly at its stated line;
// 2. else each whole hunk at the nearest offset within ±1000 lines, in order and not overlapping;
// 3. else, if every hunk matches reversed, it is already applied;
// 4. else with a base (the stored `<+>` text, or `(revision h)` of a non-git patch),
//    `merge3::try_resolve`, and a conflict for the merge window when that fails;
// 5. with no base, the file fails.
// Decision 3: a binary entry is applied by `git apply --binary` on a patch holding only it.
// Decision 5: paths are relative to the repository root, after the strip and the base directory.
//
// Everything a patch says is untrusted: every target goes through `snapshots::check_path`, a
// file mode must be one of git's, and nothing from the patch reaches git as an option.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::branches::Repo;
use crate::commands::ApiError;
use crate::git::{self, GitError};
use crate::merge3;
use crate::operation;
use crate::patch::{
    self, BinaryFilePatch, FilePatch, Hunk, Line, LineKind, Patch, PatchError, TextFilePatch,
    WriteOptions,
};
use crate::shelf::tab::{self as shelf_tab, ShelfFile};
use crate::shelf::unshelve::{self, HeadChanged};
use crate::shelf::ShelvedBinary;
use crate::snapshots::{self, EntryKind, SnapshotId};
use crate::storage;
use crate::worktree_diff::content_hash;

// The reason of the recoverable copy, `patch.apply.dialog.title` (V:346).
const APPLY_REASON: &str = "Apply Patch";
// `GenericPatchApplier.ourMaxWalk`: how far a hunk is searched from its stated line.
const MAX_WALK: usize = 1000;
// The modes git writes (`PatchUtil.REGULAR_FILE_MODE`, `EXECUTABLE_FILE_MODE`) and a link.
const REGULAR_FILE_MODE: u32 = 0o100644;
const EXECUTABLE_FILE_MODE: u32 = 0o100755;
const SYMLINK_MODE: u32 = 0o120000;
const OWNER_EXECUTE: u32 = 0o100;
const APPLY_BINARY: [&str; 3] = ["apply", "--whitespace=nowarn", "--binary"];

fn valid_mode(mode: u32) -> bool {
    [REGULAR_FILE_MODE, EXECUTABLE_FILE_MODE, SYMLINK_MODE].contains(&mode)
}

// VcsBundle.properties: `cannot.find.file.to.patch` (651), `cannot.apply.file.already.exists`
// (652) and `patch.apply.file.type.directory.error` (370).
fn not_found(path: &str) -> String {
    format!("Cannot find file to patch: {path}")
}

fn already_exists(path: &str) -> String {
    format!("Cannot apply patch to file {path}: it already exists.")
}

fn is_directory(path: &str) -> String {
    format!("Cannot apply content for {path} file from patch because it is directory.")
}

// `cannot.create.directory.for.patch` (650), with Ramify's own reason.
fn cannot_create(path: &str) -> String {
    format!("Cannot create file: {path}. One of its folders is not a folder")
}

// Ramify's own: what IntelliJ does not check or does not say.
fn changed_while_applying(path: &str) -> String {
    format!("{path} changed while the patch was being applied: it was not overwritten")
}

fn not_regular(path: &str) -> String {
    format!("{path} is not a regular file: the patch is not applied to it")
}

fn unknown_mode(path: &str, mode: u32) -> String {
    format!("The patch gives {path} the unknown file mode {mode:o}")
}

fn binary_link(path: &str) -> String {
    format!("The patch makes {path} a symbolic link: it is not applied")
}

fn not_text(path: &str) -> String {
    format!("{path} is not UTF-8 text: its changes cannot be merged")
}

const FILE_CHANGED: &str = "The file has changed since the conflict was shown: it was not written";

// Where the patch comes from. Tagged so that more sources fit.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PatchInput {
    // A patch file chosen by the user; a relative path is taken from the repository root.
    File {
        path: String,
    },
    Text {
        text: String,
    },
    // A list of the shelf, with its stored bases (Unshelve): only `files` when given. Its binary
    // files are whole copies, not patch entries: they come after the patch's files in the plan.
    Shelf {
        name: String,
        files: Option<Vec<ShelfFile>>,
    },
}

// `Remove/Restore Leading Directory` and `Map Base Directory…`, per file of the patch, by its
// index. A file beyond the list keeps its own path. `base` is a folder relative to the
// repository root (empty: the root).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PathMapping {
    pub files: Vec<FileMapping>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FileMapping {
    pub strip: usize,
    pub base: String,
}

impl PathMapping {
    fn of(&self, index: usize) -> FileMapping {
        self.files.get(index).cloned().unwrap_or_default()
    }
}

// What a file of the plan would do. `Missing` is the file to change, move or delete that is not
// there (`missing base` and `Select missing base` in the dialog); `MissingBase` is a file whose
// changes do not match and that has no base to merge with; `Exists` is a file to create (or the
// target of a move) that is already there; `Invalid` is a target the patch may not touch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Applicable,
    AlreadyApplied,
    MissingBase,
    Missing,
    Exists,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Moved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanAuthor {
    pub name: String,
    pub email: String,
}

// One file of the patch. `path` is what the dialog shows and what `selected` and `overwrite`
// name: the target after the move, or the file itself. `original` is the patch's own path and
// `strip` the leading folders taken from it (at most `max_strip`). `base` says that a base is
// there to compare with (Show Difference).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedFile {
    pub index: usize,
    pub path: String,
    pub before: Option<String>,
    pub after: Option<String>,
    pub original: String,
    pub strip: usize,
    pub max_strip: usize,
    pub kind: ChangeKind,
    pub binary: bool,
    pub base: bool,
    pub status: Status,
    pub error: Option<String>,
    // The content hash of each file it read (`ABSENT` for none): `apply` takes them back as
    // `expected`, and a file that changed since the plan is not written.
    pub hashes: BTreeMap<String, String>,
    // For a shelved list, which of its files this is, as the shelf names it.
    pub shelf: Option<ShelfFile>,
    // An invalid binary file of a shelved list whose only problem is that HEAD changed it since it
    // was shelved: the dialog offers `Overwrite` for it, which puts it in `overwrite`.
    pub overwritable: bool,
}

// The hash of a file that is not there.
pub const ABSENT: &str = "absent";

// `read_error` is the message of a patch that cannot be read (`Cannot read patch {0}: {1}`,
// V:392, is the caller's); then there are no files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyPlan {
    pub subject: Option<String>,
    pub message: Option<String>,
    pub author: Option<PlanAuthor>,
    pub additional_info_error: Option<String>,
    pub read_error: Option<String>,
    pub files: Vec<PlannedFile>,
}

// `ApplyPatchStatus`, in the order of `ORDERED_TYPES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApplyStatus {
    Skip,
    Success,
    AlreadyApplied,
    Partial,
    Failure,
    Abort,
}

impl ApplyStatus {
    // `ApplyPatchStatus.and`: the later of the two, except that SUCCESS with ALREADY_APPLIED is
    // PARTIAL "by historical reasons".
    pub fn and(lhs: Option<Self>, rhs: Option<Self>) -> Option<Self> {
        let (lhs, rhs) = match (lhs, rhs) {
            (Some(lhs), Some(rhs)) => (lhs, rhs),
            (lhs, rhs) => return lhs.or(rhs),
        };
        let partial = [Self::Success, Self::AlreadyApplied];
        if lhs != rhs && partial.contains(&lhs) && partial.contains(&rhs) {
            Some(Self::Partial)
        } else {
            Some(lhs.max(rhs))
        }
    }
}

// A file whose changes need the merge window: the three texts with `\n`, and the hash of the
// local file as it was read (`patch_conflict_resolve` writes only over that). `moved_to` and
// `new_mode` are what is still to do once it is resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchConflict {
    pub path: String,
    pub base: String,
    pub local: String,
    pub patched: String,
    pub local_hash: String,
    pub moved_to: Option<String>,
    pub new_mode: Option<u32>,
}

// `status` folds the files that were applied, skipped or failed as `executePatchGroup` does;
// the conflicts are not in it: the caller folds each one's outcome (`and`) once the merge
// window answers. `failed` names the files as `askToRollback` does (the target, else the file),
// and `messages` are the errors IntelliJ shows in its `Apply Patch` dialog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyResult {
    pub snapshot: Option<SnapshotId>,
    pub status: ApplyStatus,
    pub failed: Vec<String>,
    pub conflicts: Vec<PatchConflict>,
    pub messages: Vec<String>,
}

// The two texts of `Show Difference` and their titles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchPreview {
    pub before: String,
    pub after: String,
    pub titles: [String; 2],
}

// DiffBundle `merge.version.title.base` and VcsBundle `patch.apply.conflict.patched.version`
// (V:354).
const BASE_VERSION: &str = "Base version";
const PATCHED_VERSION: &str = "Patched version";

// What each file of the patch would do, with no writes.
pub fn plan(repo: &Path, patch: &Patch, mapping: &PathMapping) -> ApplyPlan {
    ApplyPlan {
        subject: patch.header.subject.clone(),
        message: patch.header.message.clone(),
        author: patch.header.author.as_ref().map(|author| PlanAuthor {
            name: author.name.clone(),
            email: author.email.clone(),
        }),
        additional_info_error: patch.additional_info_error.clone(),
        read_error: None,
        files: patch
            .files
            .iter()
            .enumerate()
            .map(|(index, file)| planned(repo, index, file, &mapping.of(index)))
            .collect(),
    }
}

// `plan` for a patch still to be read. `data` is the repository's data folder, where the shelf is.
pub fn plan_input(
    repo: &Path,
    data: &Path,
    source: &PatchInput,
    mapping: &PathMapping,
) -> Result<ApplyPlan, GitError> {
    Ok(match read_source(repo, data, source) {
        Ok(input) => {
            let mut planned = plan(repo, &input.patch, mapping);
            if input.shelf {
                for (file, entry) in planned.files.iter_mut().zip(&input.patch.files) {
                    file.shelf = Some(ShelfFile::of_patch(entry));
                }
                let first = input.patch.files.len();
                for (offset, binary) in input.binaries.iter().enumerate() {
                    let file = planned_binary(repo, first + offset, binary, &input.base_ids);
                    planned.files.push(file);
                }
            }
            planned
        }
        Err(message) => ApplyPlan {
            subject: None,
            message: None,
            author: None,
            additional_info_error: None,
            read_error: Some(message),
            files: Vec::new(),
        },
    })
}

// `Show Difference` of one planned file (by its `index`), read again from its source.
pub fn preview_input(
    repo: &Path,
    data: &Path,
    source: &PatchInput,
    mapping: &PathMapping,
    index: usize,
) -> Result<PatchPreview, GitError> {
    let input = read_source(repo, data, source).map_err(GitError::Io)?;
    if let Some(binary) = index
        .checked_sub(input.patch.files.len())
        .and_then(|offset| input.binaries.get(offset))
    {
        return Err(GitError::Io(binary_diff(&unshelve::binary_shown(binary))));
    }
    preview(repo, &input.patch, mapping, index).map_err(GitError::Io)
}

// `TextFilePatchInProgress.getDiffRequestProducers`: the file as it is now against what the patch
// makes of it, with the titles of `ChangeDiffRequestProducer` (`Base version` for the current
// content, the patch's after revision or `Patched version`). Not copied: the three-sided diff of
// a file that needs the merge window, which shows its two sides here. Nothing is written.
pub fn preview(
    repo: &Path,
    patch: &Patch,
    mapping: &PathMapping,
    index: usize,
) -> Result<PatchPreview, String> {
    let file = patch
        .files
        .get(index)
        .ok_or_else(|| format!("The patch has no file {index}"))?;
    let (target, error) = target_of(repo, index, file, &mapping.of(index));
    if let Some(error) = error {
        return Err(error);
    }
    let (before_name, after_name, _) = names(file);
    // `changes.error.cannot.find.base.for.path` (V:1048).
    let no_base = || {
        let name = before_name.or(after_name).unwrap_or_default();
        format!("Cannot find base for '{name}'")
    };
    let text = match file {
        FilePatch::Text(text) => text,
        FilePatch::Binary(_) => return Err(binary_diff(&target.path)),
    };
    let titles = [
        BASE_VERSION.to_string(),
        text.after_rev
            .clone()
            .unwrap_or_else(|| PATCHED_VERSION.to_string()),
    ];
    let read = |path: &str| -> Result<Option<Vec<u8>>, String> {
        contents(repo, path).map_err(|error| error.to_string())
    };
    let shown = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    let (before, after) = match target.kind {
        ChangeKind::Added => {
            let content = new_file_content(text).ok_or_else(no_base)?;
            (String::new(), shown(&content))
        }
        ChangeKind::Deleted => {
            let local = read(target.before())?.ok_or_else(no_base)?;
            (shown(&local), String::new())
        }
        ChangeKind::Modified | ChangeKind::Moved => {
            let local = to_lf(&read(target.before())?.ok_or_else(no_base)?);
            let after = match evaluate(repo, text, target.before(), &local) {
                Outcome::Patched(patched) | Outcome::Merged(patched) => shown(&patched),
                Outcome::AlreadyApplied => shown(&local),
                Outcome::Conflict { patched, .. } => patched,
                Outcome::NotText => return Err(binary_diff(&target.path)),
                Outcome::NoBase => return Err(no_base()),
            };
            (shown(&local), after)
        }
    };
    Ok(PatchPreview {
        before,
        after,
        titles,
    })
}

// `changes.error.can.t.show.diff.for.binary.file` (V:1049).
fn binary_diff(path: &str) -> String {
    format!("Cannot show diff for binary file '{path}'")
}

// Applies the selected files (by their planned `path`). New files whose target exists are
// written only when they are in `overwrite`; the others are skipped, as when IntelliJ's
// `Overwrite Existing Files` leaves them out. A copy of every file it may touch is taken
// before the first write.
pub fn apply(
    repo: &Repo,
    patch: &Patch,
    mapping: &PathMapping,
    selected: &[String],
    overwrite: &[String],
    expected: &BTreeMap<String, String>,
) -> Result<ApplyResult, GitError> {
    let files = Files {
        selected,
        overwrite,
        expected,
    };
    apply_watching(repo, patch, mapping, &files, &mut |_, _| {})
}

// What the dialog chose: the files (by their planned `path`), the new files to write over, and
// the hashes the plan read.
struct Files<'a> {
    selected: &'a [String],
    overwrite: &'a [String],
    expected: &'a BTreeMap<String, String>,
}

// The same, as the dialog sends it: `expected` is the plan's `hashes`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ApplyChoice {
    pub selected: Vec<String>,
    pub overwrite: Vec<String>,
    pub expected: BTreeMap<String, String>,
}

// When `apply_watching` lets the tests act: right before a file is read after the copy, and
// right before it is checked and written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Moment {
    Read,
    Write,
}

// `apply` for a patch still to be read, only if HEAD is still the one the dialog saw.
pub fn apply_input(
    repo: &Repo,
    source: &PatchInput,
    mapping: &PathMapping,
    choice: &ApplyChoice,
    expected_head: &str,
) -> Result<ApplyResult, GitError> {
    operation::check_head(repo.path, expected_head)?;
    let input = read_source(repo.path, repo.data, source).map_err(GitError::Io)?;
    let ApplyChoice {
        selected,
        overwrite,
        expected,
    } = choice;
    let mut result = apply(repo, &input.patch, mapping, selected, overwrite, expected)?;
    // The binary files of a shelved list go back from their copies, after the texts.
    for binary in &input.binaries {
        let path = unshelve::binary_shown(binary);
        if !selected.contains(&path) {
            continue;
        }
        let head_changed = match overwrite.contains(&path) {
            true => HeadChanged::Overwrite,
            false => HeadChanged::Refuse,
        };
        let put = unshelve::put_back(repo, binary, &input.base_ids, Some(expected), head_changed);
        let outcome = match put {
            Ok(_) => ApplyStatus::Success,
            Err(message) => {
                result.messages.push(message);
                result.failed.push(path);
                ApplyStatus::Failure
            }
        };
        result.status =
            ApplyStatus::and(Some(result.status), Some(outcome)).unwrap_or(ApplyStatus::Failure);
    }
    Ok(result)
}

// The hash `plan` gives a file of the repository as it is now (`ABSENT` when it is not there).
pub(crate) fn hash_at(repo: &Path, path: &str) -> io::Result<String> {
    Ok(hash_of(contents(repo, path)?.as_deref()))
}

// A source as read: the patch and, for a shelved list, its binary files and what they were
// shelved over.
struct Source {
    patch: Patch,
    shelf: bool,
    binaries: Vec<ShelvedBinary>,
    base_ids: BTreeMap<String, String>,
}

fn read_source(repo: &Path, data: &Path, source: &PatchInput) -> Result<Source, String> {
    if let PatchInput::Shelf { name, files } = source {
        let shelved = shelf_tab::source(data, name, files.as_deref())?;
        return Ok(Source {
            patch: shelved.patch,
            shelf: true,
            binaries: shelved.binaries,
            base_ids: shelved.base_ids,
        });
    }
    Ok(Source {
        patch: read_input(repo, source)?,
        shelf: false,
        binaries: Vec::new(),
        base_ids: BTreeMap::new(),
    })
}

// A binary file of a shelved list as the dialog shows it. What `put_back` would refuse makes it
// invalid, with the reason: a binary file is never written over (no `Overwrite Existing Files`),
// except one that HEAD changed since it was shelved, which the dialog may mark to overwrite.
fn planned_binary(
    repo: &Path,
    index: usize,
    binary: &ShelvedBinary,
    base_ids: &BTreeMap<String, String>,
) -> PlannedFile {
    let path = unshelve::binary_shown(binary);
    let kind = match (&binary.before, &binary.after) {
        (None, _) => ChangeKind::Added,
        (_, None) => ChangeKind::Deleted,
        (Some(before), Some(after)) if before != after => ChangeKind::Moved,
        _ => ChangeKind::Modified,
    };
    let missing = binary
        .before
        .as_ref()
        .filter(|before| snapshots::check_path(repo, before).is_ok())
        .is_some_and(|before| !repo.join(before).is_file());
    let (status, error) = if missing {
        (Status::Missing, None)
    } else {
        match unshelve::binary_problem(repo, binary, base_ids, HeadChanged::Refuse) {
            Ok(()) => (Status::Applicable, None),
            Err(message) => (Status::Invalid, Some(message)),
        }
    };
    let overwritable = status == Status::Invalid
        && unshelve::binary_problem(repo, binary, base_ids, HeadChanged::Overwrite).is_ok();
    let hashes = if status == Status::Invalid && !overwritable {
        BTreeMap::new()
    } else {
        unshelve::binary_paths(binary)
            .into_iter()
            .filter_map(|path| Some((path.clone(), hash_at(repo, &path).ok()?)))
            .collect()
    };
    PlannedFile {
        index,
        path: path.clone(),
        before: binary.before.clone(),
        after: binary.after.clone(),
        original: path,
        strip: 0,
        max_strip: 0,
        kind,
        binary: true,
        base: false,
        status,
        error,
        hashes,
        shelf: Some(ShelfFile::of_binary(binary)),
        overwritable,
    }
}

// Import to Shelf of the Apply Patch dialog (`ImportToShelfExecutor`): the included files, with
// the paths the dialog maps them to and the bases the patch has.
pub fn mapped_patch(
    repo: &Path,
    source: &PatchInput,
    mapping: &PathMapping,
    selected: &[String],
) -> Result<Patch, GitError> {
    let mut patch = read_input(repo, source).map_err(GitError::Io)?;
    let mut files = Vec::new();
    for (index, file) in patch.files.iter().enumerate() {
        let (target, error) = target_of(repo, index, file, &mapping.of(index));
        if !selected.contains(&target.path) {
            continue;
        }
        if let Some(error) = error {
            return Err(GitError::Io(error));
        }
        let mut file = file.clone();
        match &mut file {
            FilePatch::Text(text) => {
                text.before = target.before.clone();
                text.after = target.after.clone();
            }
            FilePatch::Binary(binary) => {
                binary.before = target.before.clone();
                binary.after = target.after.clone();
            }
        }
        files.push(file);
    }
    patch.files = files;
    Ok(patch)
}

fn read_input(repo: &Path, source: &PatchInput) -> Result<Patch, String> {
    let bytes = match source {
        PatchInput::Shelf { .. } => return Err("A shelved list is read with its store".to_string()),
        PatchInput::Text { text } => return patch::read(text.as_bytes()).map_err(error_text),
        PatchInput::File { path } => {
            let file = Path::new(path);
            let file = if file.is_absolute() {
                file.to_path_buf()
            } else {
                repo.join(file)
            };
            match fs::read(file) {
                Ok(bytes) => bytes,
                // `patch.apply.can.t.find.patch.file.warning` (V:387).
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Err(format!("Cannot find patch file {path}"))
                }
                Err(error) => return Err(error.to_string()),
            }
        }
    };
    patch::read(&bytes).map_err(error_text)
}

fn error_text(error: PatchError) -> String {
    match error {
        PatchError::Syntax { message, .. } => message,
        PatchError::Git(error) => ApiError::from(error).message,
        PatchError::BinaryAsText { path } => format!("{path} is not a text file"),
    }
}

//
// Targets
//

#[derive(Debug, Clone)]
struct Target {
    index: usize,
    kind: ChangeKind,
    before: Option<String>,
    after: Option<String>,
    path: String,
    original: String,
    strip: usize,
    max_strip: usize,
    new_mode: Option<u32>,
}

impl Target {
    fn before(&self) -> &str {
        self.before.as_deref().unwrap_or_default()
    }

    fn after(&self) -> &str {
        self.after.as_deref().unwrap_or_default()
    }

    // The files it reads or writes.
    fn paths(&self) -> impl Iterator<Item = &String> {
        self.before.iter().chain(
            self.after
                .iter()
                .filter(|after| self.before.as_ref() != Some(*after)),
        )
    }
}

fn names(file: &FilePatch) -> (Option<&str>, Option<&str>, Option<u32>) {
    match file {
        FilePatch::Text(text) => (text.before.as_deref(), text.after.as_deref(), text.new_mode),
        FilePatch::Binary(binary) => (
            binary.before.as_deref(),
            binary.after.as_deref(),
            binary.new_mode,
        ),
    }
}

// `StripCapablePath`: the name without its first `strip` folders (never its last part), and how
// many folders it has.
fn stripped(name: &str, strip: usize) -> (String, usize) {
    let corrected = name.trim_matches(|c: char| c <= ' ').replace('\\', "/");
    let steps: Vec<&str> = corrected.split('/').collect();
    let max = steps.len() - 1;
    let start: usize = steps[..strip.min(max)]
        .iter()
        .map(|step| step.len() + 1)
        .sum();
    (corrected[start..].to_string(), max)
}

// The target in the repository, and whether the patch may touch it. Written with `/` and no empty
// parts, so that it can be compared with what the dialog sends back.
fn mapped(repo: &Path, name: &str, mapping: &FileMapping) -> (String, usize, Result<(), String>) {
    let (path, max) = stripped(name, mapping.strip);
    let base = mapping.base.trim_end_matches('/');
    let joined = if base.is_empty() {
        path
    } else {
        format!("{base}/{path}")
    };
    if let Err(error) = snapshots::check_path(repo, &joined) {
        return (joined, max, Err(error.to_string()));
    }
    let normal: Vec<&str> = Path::new(&joined)
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect();
    (normal.join("/"), max, Ok(()))
}

// `PatchStrippable` and `PathsVerifier.getChecker`. The error is why the patch may not touch it.
fn target_of(
    repo: &Path,
    index: usize,
    file: &FilePatch,
    mapping: &FileMapping,
) -> (Target, Option<String>) {
    let (before_name, after_name, new_mode) = names(file);
    let mut error = None;
    let mut max_strip = usize::MAX;
    let mut map = |name: Option<&str>| {
        name.map(|name| {
            let (path, max, checked) = mapped(repo, name, mapping);
            max_strip = max_strip.min(max);
            if let Err(message) = checked {
                error.get_or_insert(message);
            }
            path
        })
    };
    let after = map(after_name);
    let before = map(before_name);
    let kind = match (&before, &after) {
        (None, _) => ChangeKind::Added,
        (_, None) => ChangeKind::Deleted,
        (Some(before), Some(after)) if before != after => ChangeKind::Moved,
        _ => ChangeKind::Modified,
    };
    let path = after.clone().or_else(|| before.clone()).unwrap_or_default();
    match new_mode {
        Some(mode) if !valid_mode(mode) => {
            error.get_or_insert(unknown_mode(&path, mode));
        }
        // `git apply` would make a real link, which the text path never does.
        Some(SYMLINK_MODE) if matches!(file, FilePatch::Binary(_)) => {
            error.get_or_insert(binary_link(&path));
        }
        _ => {}
    }
    let original = after_name.or(before_name).unwrap_or_default().to_string();
    let max_strip = if max_strip == usize::MAX {
        0
    } else {
        max_strip
    };
    let target = Target {
        index,
        kind,
        before,
        after,
        path,
        original,
        strip: mapping.strip.min(max_strip),
        max_strip,
        new_mode,
    };
    (target, error)
}

//
// The disk
//

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Absent,
    File,
    Directory,
    // A link or something else: the patch is not applied through it.
    Other,
}

fn kind(repo: &Path, path: &str) -> io::Result<Kind> {
    match repo.join(path).symlink_metadata() {
        Ok(meta) if meta.is_dir() => Ok(Kind::Directory),
        Ok(meta) if meta.is_file() => Ok(Kind::File),
        Ok(_) => Ok(Kind::Other),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(Kind::Absent)
        }
        Err(error) => Err(error),
    }
}

// The bytes of a regular file, or nothing if it is not there. Anything else is not what was read.
fn contents(repo: &Path, path: &str) -> io::Result<Option<Vec<u8>>> {
    match kind(repo, path)? {
        Kind::Absent => Ok(None),
        Kind::File => fs::read(repo.join(path)).map(Some),
        Kind::Directory | Kind::Other => Err(io::Error::other(not_regular(path))),
    }
}

pub(crate) fn hash_of(bytes: Option<&[u8]>) -> String {
    bytes.map_or_else(|| ABSENT.to_string(), content_hash)
}

// What `PathsVerifier.nonWriteActionPreCheck` says of a file, plus Ramify's own refusal of
// directories and links (`filterBadFileTypePatches` for directories).
enum Check {
    Go,
    // A deleted file that is already gone.
    Skip,
    // A new file whose target exists, or the target of a move.
    Exists(String),
    Missing(String),
    Fail(String),
}

fn regular(repo: &Path, path: &str) -> io::Result<Result<bool, String>> {
    Ok(match kind(repo, path)? {
        Kind::Absent => Ok(false),
        Kind::File => Ok(true),
        Kind::Directory => Err(is_directory(path)),
        Kind::Other => Err(not_regular(path)),
    })
}

fn precheck(repo: &Path, target: &Target) -> io::Result<Check> {
    // A file to create under a folder that is now a file or a link.
    let created = match target.kind {
        ChangeKind::Added | ChangeKind::Moved => target.after.as_deref(),
        ChangeKind::Modified | ChangeKind::Deleted => None,
    };
    if let Some(path) = created.filter(|path| snapshots::behind_other(repo, path)) {
        return Ok(Check::Fail(cannot_create(path)));
    }
    let check = match target.kind {
        ChangeKind::Added => match regular(repo, target.after())? {
            Ok(false) => Check::Go,
            Ok(true) => Check::Exists(already_exists(target.after())),
            Err(message) => Check::Fail(message),
        },
        ChangeKind::Deleted => match regular(repo, target.before())? {
            Ok(false) => Check::Skip,
            Ok(true) => Check::Go,
            Err(message) => Check::Fail(message),
        },
        ChangeKind::Modified | ChangeKind::Moved => match regular(repo, target.before())? {
            Ok(false) => Check::Missing(not_found(target.before())),
            Err(message) => Check::Fail(message),
            Ok(true) if target.kind == ChangeKind::Modified => Check::Go,
            Ok(true) => match regular(repo, target.after())? {
                Ok(false) => Check::Go,
                Ok(true) => Check::Exists(already_exists(target.after())),
                Err(message) => Check::Fail(message),
            },
        },
    };
    Ok(check)
}

//
// The plan
//

fn planned(repo: &Path, index: usize, file: &FilePatch, mapping: &FileMapping) -> PlannedFile {
    let (target, error) = target_of(repo, index, file, mapping);
    let mut hashes = BTreeMap::new();
    if error.is_none() {
        for path in target.paths() {
            if let Ok(bytes) = contents(repo, path) {
                hashes.insert(path.clone(), hash_of(bytes.as_deref()));
            }
        }
    }
    let (status, error) = match error {
        Some(error) => (Status::Invalid, Some(error)),
        None => match status_of(repo, &target, file) {
            Ok(status) => (status, None),
            Err(error) => (Status::Invalid, Some(error)),
        },
    };
    let base = match file {
        FilePatch::Text(text) => text.base.is_some() || revision_of(text).is_some(),
        FilePatch::Binary(_) => false,
    };
    PlannedFile {
        index: target.index,
        path: target.path.clone(),
        before: target.before.clone(),
        after: target.after.clone(),
        original: target.original.clone(),
        strip: target.strip,
        max_strip: target.max_strip,
        kind: target.kind,
        binary: matches!(file, FilePatch::Binary(_)),
        base,
        status,
        error,
        hashes,
        shelf: None,
        overwritable: false,
    }
}

fn status_of(repo: &Path, target: &Target, file: &FilePatch) -> Result<Status, String> {
    match precheck(repo, target).map_err(|error| error.to_string())? {
        // The dialog shows a deleted file that is gone as `missing base`; applied, it is skipped.
        Check::Skip | Check::Missing(_) => return Ok(Status::Missing),
        Check::Exists(_) => return Ok(Status::Exists),
        Check::Fail(message) => return Err(message),
        Check::Go => {}
    }
    let text = match file {
        FilePatch::Binary(binary) => {
            let entry = binary_entry(target, binary);
            let check = |args: &[&str]| git::run_with_input(repo, args, &entry).is_ok();
            return Ok(if check(&[&APPLY_BINARY[..], &["--check"]].concat()) {
                Status::Applicable
            } else if check(&[&APPLY_BINARY[..], &["-R", "--check"]].concat()) {
                Status::AlreadyApplied
            } else {
                Status::MissingBase
            });
        }
        FilePatch::Text(text) => text,
    };
    match target.kind {
        ChangeKind::Added if new_file_content(text).is_none() => return Ok(Status::MissingBase),
        ChangeKind::Added | ChangeKind::Deleted => return Ok(Status::Applicable),
        ChangeKind::Modified | ChangeKind::Moved => {}
    }
    let local = contents(repo, target.before())
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    Ok(
        match evaluate(repo, text, target.before(), &to_lf(&local)) {
            Outcome::Patched(_) | Outcome::Merged(_) | Outcome::Conflict { .. } => {
                Status::Applicable
            }
            Outcome::AlreadyApplied => Status::AlreadyApplied,
            Outcome::NotText | Outcome::NoBase => Status::MissingBase,
        },
    )
}

//
// One text: decision 4
//

enum Outcome {
    Patched(Vec<u8>),
    AlreadyApplied,
    Merged(Vec<u8>),
    Conflict {
        base: String,
        local: String,
        patched: String,
    },
    // A base was found, but the texts are not UTF-8: they cannot be merged.
    NotText,
    NoBase,
}

// `local` and the result have `\n` as their only separator.
fn evaluate(repo: &Path, file: &TextFilePatch, before: &str, local: &[u8]) -> Outcome {
    if file.hunks.is_empty() {
        return Outcome::Patched(local.to_vec());
    }
    if let Some(patched) = apply_hunks(local, &file.hunks) {
        return Outcome::Patched(patched);
    }
    let reversed: Vec<Hunk> = file.hunks.iter().map(reversed).collect();
    // A reversed hunk with nothing to find (a deletion without context) would match anywhere.
    let findable = reversed
        .iter()
        .all(|hunk| hunk.lines.iter().any(|line| line.kind != LineKind::Added));
    if findable && apply_hunks(local, &reversed).is_some() {
        return Outcome::AlreadyApplied;
    }
    // `ApplyPatchForBaseRevisionTexts.create`: the stored base, then the revision. A base the
    // hunks do not apply to is wrong (V:405) and the next one is tried.
    let stored = || file.base.clone();
    let revision = || revision_base(repo, file, before);
    for base in [&stored as &dyn Fn() -> Option<Vec<u8>>, &revision] {
        let Some(base) = base() else { continue };
        let base = to_lf(&base);
        if let Some(patched) = apply_hunks(&base, &file.hunks) {
            return merge(base, local, patched);
        }
    }
    Outcome::NoBase
}

fn merge(base: Vec<u8>, local: &[u8], patched: Vec<u8>) -> Outcome {
    // `ApplyPatchAction.applyContent`: local equals patched.
    if local == patched {
        return Outcome::AlreadyApplied;
    }
    let (Ok(base), Ok(local), Ok(patched)) = (
        String::from_utf8(base),
        std::str::from_utf8(local),
        String::from_utf8(patched),
    ) else {
        return Outcome::NotText;
    };
    match merge3::try_resolve(local, &base, &patched) {
        Some(merged) => Outcome::Merged(merged.into_bytes()),
        None => Outcome::Conflict {
            base,
            local: local.to_string(),
            patched,
        },
    }
}

// `DefaultPatchBaseVersionProvider`: `ourRevisionPatternTemplate` with git's revision pattern.
static REVISION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(revision ([0-9a-fA-F]+)\)").unwrap());

fn revision_of(file: &TextFilePatch) -> Option<&str> {
    let id = file.before_rev.as_deref()?;
    Some(REVISION.captures(id)?.get(1)?.as_str())
}

// The content at that commit (`GitHistoryProvider.getBaseVersionContent`). Not copied: the search
// of the file's history for an older revision, and `(date N)`.
fn revision_base(repo: &Path, file: &TextFilePatch, before: &str) -> Option<Vec<u8>> {
    let revision = revision_of(file)?;
    let commit = git::run(
        repo,
        &[
            "rev-parse",
            "-q",
            "--verify",
            &format!("{revision}^{{commit}}"),
        ],
    )
    .ok()?;
    let commit = String::from_utf8(commit).ok()?;
    git::run(
        repo,
        &["cat-file", "blob", &format!("{}:{before}", commit.trim())],
    )
    .ok()
}

fn reversed(hunk: &Hunk) -> Hunk {
    Hunk {
        before_start: hunk.after_start,
        before_count: hunk.after_count,
        after_start: hunk.before_start,
        after_count: hunk.before_count,
        lines: hunk
            .lines
            .iter()
            .map(|line| Line {
                kind: match line.kind {
                    LineKind::Added => LineKind::Removed,
                    LineKind::Removed => LineKind::Added,
                    LineKind::Context => LineKind::Context,
                },
                ..line.clone()
            })
            .collect(),
    }
}

// The lines of `LineOffsetsUtil.create`: `"a\nb\n"` has three, the last one empty.
fn split_lines(text: &[u8]) -> Vec<&[u8]> {
    text.split(|byte| *byte == b'\n').collect()
}

// Steps 1 and 2 of decision 4: each hunk at its stated line (offset 0) or at the nearest offset.
pub(crate) fn apply_hunks(text: &[u8], hunks: &[Hunk]) -> Option<Vec<u8>> {
    let lines = split_lines(text);
    let starts = place(&lines, text.is_empty(), hunks)?;
    Some(render(&lines, text.is_empty(), hunks, &starts))
}

// Where each hunk starts (from 0), in order and not overlapping. The counts must match the
// lines (`checkContextLines`), and every context and removed line must equal its line.
fn place(lines: &[&[u8]], empty: bool, hunks: &[Hunk]) -> Option<Vec<usize>> {
    let mut starts = Vec::with_capacity(hunks.len());
    let mut first_free = 0;
    for hunk in hunks {
        let base: Vec<&[u8]> = hunk
            .lines
            .iter()
            .filter(|line| line.kind != LineKind::Added)
            .map(|line| line.text.as_slice())
            .collect();
        let patched = hunk
            .lines
            .iter()
            .filter(|line| line.kind != LineKind::Removed)
            .count();
        if base.len() != hunk.before_count || patched != hunk.after_count {
            return None;
        }
        // `-s,0` inserts after line `s`, as git (IntelliJ inserts after `s - 1`); in an empty
        // text there is only the start.
        let stated = if hunk.before_count > 0 {
            hunk.before_start.checked_sub(1)?
        } else if empty {
            0
        } else {
            hunk.before_start
        };
        let fits = |start: usize| {
            start >= first_free
                && start
                    .checked_add(base.len())
                    .is_some_and(|end| end <= lines.len())
                && base
                    .iter()
                    .zip(&lines[start..])
                    .all(|(want, line)| want == line)
        };
        let start = if base.is_empty() {
            Some(stated).filter(|start| fits(*start))
        } else {
            (0..=MAX_WALK).find_map(|offset| {
                [stated.checked_sub(offset), stated.checked_add(offset)]
                    .into_iter()
                    .flatten()
                    .find(|start| fits(*start))
            })
        }?;
        starts.push(start);
        first_free = start + base.len();
    }
    Some(starts)
}

struct Output {
    bytes: Vec<u8>,
    lines: usize,
}

impl Output {
    // `appendLine`.
    fn push(&mut self, line: &[u8]) {
        if self.lines > 0 {
            self.bytes.push(b'\n');
        }
        self.bytes.extend_from_slice(line);
        self.lines += 1;
    }

    fn end_with_newline(&mut self, no_newline: bool) {
        if !no_newline && self.lines > 0 {
            self.bytes.push(b'\n');
        }
    }
}

// `PlainSimplePatchApplier.execute` with the starts found by `place`.
fn render(lines: &[&[u8]], empty: bool, hunks: &[Hunk], starts: &[usize]) -> Vec<u8> {
    let mut out = Output {
        bytes: Vec::new(),
        lines: 0,
    };
    let mut base_line = 0;
    for (hunk, &start) in hunks.iter().zip(starts) {
        for line in &lines[base_line..start] {
            out.push(line);
        }
        for line in hunk
            .lines
            .iter()
            .filter(|line| line.kind != LineKind::Removed)
        {
            out.push(&line.text);
        }
        base_line = start + hunk.before_count;
    }
    // `handleLastLine`.
    let last = hunks.last().map_or(&[][..], |hunk| hunk.lines.as_slice());
    let last_base = last.iter().rev().find(|line| line.kind != LineKind::Added);
    let last_patched = last
        .iter()
        .rev()
        .find(|line| line.kind != LineKind::Removed);
    let handled = match last_base {
        Some(base) => {
            let at_end =
                (!base.no_newline && base_line + 1 == lines.len() && lines[base_line].is_empty())
                    || (base.no_newline && base_line == lines.len());
            if at_end {
                out.end_with_newline(last_patched.map_or(base.no_newline, |line| line.no_newline));
            }
            at_end
        }
        // An insertion into an empty file: the `\ No newline` of the patch decides.
        None if base_line == 0 && empty => {
            out.end_with_newline(last_patched.is_some_and(|line| line.no_newline));
            true
        }
        None => false,
    };
    if !handled {
        for line in &lines[base_line..] {
            out.push(line);
        }
    }
    out.bytes
}

// `TextFilePatch.getSingleHunkPatchText` and `PatchHunk.getText`.
fn new_file_content(file: &TextFilePatch) -> Option<Vec<u8>> {
    match file.hunks.as_slice() {
        [] => Some(Vec::new()),
        [hunk] => {
            let mut text = Vec::new();
            for line in &hunk.lines {
                text.extend_from_slice(&line.text);
                if !line.no_newline {
                    text.push(b'\n');
                }
            }
            Some(text)
        }
        hunks => apply_hunks(b"", hunks),
    }
}

// The separator a document keeps (`LoadTextUtil.detectSeparators`): the first one in the file.
fn separator(bytes: &[u8]) -> &'static [u8] {
    match bytes
        .iter()
        .position(|byte| *byte == b'\n' || *byte == b'\r')
    {
        Some(index) if bytes[index] == b'\r' => {
            if bytes.get(index + 1) == Some(&b'\n') {
                b"\r\n"
            } else {
                b"\r"
            }
        }
        _ => b"\n",
    }
}

fn to_lf(text: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        if text[index] == b'\r' {
            result.push(b'\n');
            if text.get(index + 1) == Some(&b'\n') {
                index += 1;
            }
        } else {
            result.push(text[index]);
        }
        index += 1;
    }
    result
}

fn with_separator(text: &[u8], separator: &[u8]) -> Vec<u8> {
    if separator == b"\n" {
        return text.to_vec();
    }
    let mut result = Vec::with_capacity(text.len());
    for byte in text {
        if *byte == b'\n' {
            result.extend_from_slice(separator);
        } else {
            result.push(*byte);
        }
    }
    result
}

// A patch with only this binary entry, under its targets, for `git apply --binary`.
fn binary_entry(target: &Target, file: &BinaryFilePatch) -> Vec<u8> {
    let entry = BinaryFilePatch {
        before: target.before.clone(),
        after: target.after.clone(),
        ..file.clone()
    };
    let patch = Patch {
        files: vec![FilePatch::Binary(entry)],
        ..Patch::default()
    };
    patch::write(&patch, &WriteOptions::default())
}

//
// Applying
//

struct Job<'a> {
    target: Target,
    file: &'a FilePatch,
}

enum Done {
    Applied(ApplyStatus),
    Conflict(PatchConflict),
    Failed(Option<String>),
}

// What `TriggerAdditionOrDeletion` tells git, silently, once the files are written.
#[derive(Default)]
struct IndexChanges {
    added: Vec<String>,
    removed: Vec<String>,
}

struct Writer<'a, 'b> {
    root: &'a Path,
    // The copy, named when a file cannot be put back.
    snapshot: SnapshotId,
    // What each file had when it was read, right after the copy.
    read: BTreeMap<String, Option<Vec<u8>>>,
    watch: &'b mut dyn FnMut(Moment, &str),
    index: IndexChanges,
    // What went wrong without failing the file (a mode that could not be set).
    notes: Vec<String>,
}

impl Writer<'_, '_> {
    fn expected(&self, path: &str) -> Option<&[u8]> {
        self.read.get(path).and_then(|bytes| bytes.as_deref())
    }

    // Review Focus 1: the file must still have what was read.
    fn unchanged(&mut self, path: &str) -> Result<(), Option<String>> {
        (self.watch)(Moment::Write, path);
        let now = contents(self.root, path).ok();
        if now.as_ref().map(|bytes| bytes.as_deref()) != Some(self.expected(path)) {
            return Err(Some(changed_while_applying(path)));
        }
        Ok(())
    }

    fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), Option<String>> {
        self.unchanged(path)?;
        let target = self.root.join(path);
        let permissions = target
            .symlink_metadata()
            .ok()
            .map(|meta| meta.permissions());
        let written = target
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| storage::write_atomic_with(&target, bytes, permissions));
        written.map_err(|error| Some(format!("{path}: {error}")))
    }

    fn remove(&mut self, path: &str) -> Result<(), Option<String>> {
        self.unchanged(path)?;
        fs::remove_file(self.root.join(path)).map_err(|error| Some(format!("{path}: {error}")))
    }

    // `doMoveIfNeeded`, after a successful apply. The index follows a tracked file.
    fn move_file(&mut self, target: &Target) -> Result<(), Option<String>> {
        if target.kind != ChangeKind::Moved {
            return Ok(());
        }
        let (before, after) = (target.before(), target.after());
        self.unchanged(after)?;
        let tracked = tracked(self.root, before);
        move_to(self.root, before, after).map_err(Some)?;
        if tracked {
            self.index.removed.push(before.to_string());
            self.index.added.push(after.to_string());
        }
        Ok(())
    }

    fn text(&mut self, job: &Job, file: &TextFilePatch) -> Done {
        let target = &job.target;
        let result = match target.kind {
            ChangeKind::Added => self.added(target, file),
            ChangeKind::Deleted => self.remove(target.before()).map(|()| {
                self.index.removed.push(target.before().to_string());
                Done::Applied(ApplyStatus::Success)
            }),
            ChangeKind::Modified | ChangeKind::Moved => self.changed(target, file),
        };
        result.unwrap_or_else(Done::Failed)
    }

    fn added(&mut self, target: &Target, file: &TextFilePatch) -> Result<Done, Option<String>> {
        let content = new_file_content(file).ok_or(None)?;
        self.write(target.after(), &content)?;
        self.notes
            .extend(set_mode(self.root, target.after(), target.new_mode));
        self.index.added.push(target.after().to_string());
        Ok(Done::Applied(ApplyStatus::Success))
    }

    fn changed(&mut self, target: &Target, file: &TextFilePatch) -> Result<Done, Option<String>> {
        let before = target.before();
        let bytes = self.expected(before).unwrap_or_default().to_vec();
        let separator = separator(&bytes);
        let status = match evaluate(self.root, file, before, &to_lf(&bytes)) {
            Outcome::Patched(patched) | Outcome::Merged(patched) => {
                let patched = with_separator(&patched, separator);
                if patched != bytes {
                    self.write(before, &patched)?;
                }
                ApplyStatus::Success
            }
            Outcome::AlreadyApplied => ApplyStatus::AlreadyApplied,
            Outcome::Conflict {
                base,
                local,
                patched,
            } => {
                return Ok(Done::Conflict(PatchConflict {
                    path: before.to_string(),
                    base,
                    local,
                    patched,
                    local_hash: content_hash(&bytes),
                    moved_to: (target.kind == ChangeKind::Moved)
                        .then(|| target.after().to_string()),
                    new_mode: target.new_mode,
                }))
            }
            Outcome::NotText => return Err(Some(not_text(before))),
            Outcome::NoBase => return Err(None),
        };
        // `applyAdditionalPatchData`, then the move.
        self.notes
            .extend(set_mode(self.root, before, target.new_mode));
        self.move_file(target)?;
        Ok(Done::Applied(status))
    }

    fn binary(&mut self, repo: &Repo, job: &Job, file: &BinaryFilePatch) -> Done {
        let target = &job.target;
        let entry = binary_entry(target, file);
        for path in target.paths() {
            if let Err(message) = self.unchanged(path) {
                return Done::Failed(message);
            }
        }
        // Overwriting a new file: git creates it only where there is none, so the old one is
        // renamed aside (bytes and mode) and renamed back if git refuses.
        let aside = match (target.kind, self.expected(target.after())) {
            (ChangeKind::Added, Some(_)) => {
                let full = self.root.join(target.after());
                match set_aside(&full) {
                    Ok(aside) => Some(aside),
                    Err(error) => {
                        return Done::Failed(Some(format!("{}: {error}", target.after())))
                    }
                }
            }
            _ => None,
        };
        let tracked = target.kind == ChangeKind::Moved && tracked(self.root, target.before());
        // As `apply_commits`: the user's `apply.whitespace=error` must not refuse it.
        let error = match repo.ctx.git_loud_with_input(&APPLY_BINARY, &entry) {
            Ok(_) => {
                match target.kind {
                    ChangeKind::Added => self.index.added.push(target.after().to_string()),
                    ChangeKind::Deleted => self.index.removed.push(target.before().to_string()),
                    ChangeKind::Moved if tracked => {
                        self.index.removed.push(target.before().to_string());
                        self.index.added.push(target.after().to_string());
                    }
                    _ => {}
                }
                if let Some(aside) = &aside {
                    if let Err(error) = fs::remove_file(aside) {
                        self.notes.push(format!("{}: {error}", aside.display()));
                    }
                }
                return Done::Applied(ApplyStatus::Success);
            }
            Err(error) => error,
        };
        if let Some(aside) = &aside {
            if let Err(message) = put_back(self.root, aside, target.after(), &self.snapshot) {
                self.notes.push(message);
            }
        }
        if git::run_with_input(
            self.root,
            &[&APPLY_BINARY[..], &["-R", "--check"]].concat(),
            &entry,
        )
        .is_ok()
        {
            return Done::Applied(ApplyStatus::AlreadyApplied);
        }
        Done::Failed(Some(ApiError::from(error).message))
    }
}

// A name next to the file, for keeping it while git writes the new one.
fn aside_numbered(full: &Path, attempt: u32) -> PathBuf {
    let mut name = std::ffi::OsString::from(".");
    name.push(full.file_name().unwrap_or_default());
    name.push(format!(".ramify-patch-{}", std::process::id()));
    if attempt > 0 {
        name.push(format!("-{attempt}"));
    }
    full.with_file_name(name)
}

const ASIDE_ATTEMPTS: u32 = 100;

// The file under a free name: the hard link is created only where nothing is (so no one's file
// is replaced) and keeps the bytes and the mode; then the old name is removed.
fn set_aside(full: &Path) -> io::Result<PathBuf> {
    for attempt in 0..ASIDE_ATTEMPTS {
        let aside = aside_numbered(full, attempt);
        match fs::hard_link(full, &aside) {
            Ok(()) => {
                if let Err(error) = fs::remove_file(full) {
                    let _ = fs::remove_file(&aside);
                    return Err(error);
                }
                return Ok(aside);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no free name to keep the file aside",
    ))
}

// The user's file back in its place; when that fails, the copy is named.
fn put_back(root: &Path, aside: &Path, path: &str, snapshot: &str) -> Result<(), String> {
    fs::rename(aside, root.join(path)).map_err(|error| {
        format!(
            "{path} could not be put back after the patch failed ({error}): its content is in the copy {snapshot}"
        )
    })
}

fn move_to(root: &Path, before: &str, after: &str) -> Result<(), String> {
    let target = root.join(after);
    if target.symlink_metadata().is_ok() {
        return Err(already_exists(after));
    }
    target
        .parent()
        .map_or(Ok(()), fs::create_dir_all)
        .and_then(|()| fs::rename(root.join(before), &target))
        .map_err(|error| format!("{after}: {error}"))
}

fn tracked(root: &Path, path: &str) -> bool {
    git::git_with_paths(root, false, &["ls-files", "-z"], &[path.to_string()])
        .is_ok_and(|output| !output.is_empty())
}

// `PatchApplier.applyAdditionalPatchData`: `File.setExecutable`, for the owner only. A link mode
// writes nothing special: the content is the link's text, as in IntelliJ.
fn set_mode(root: &Path, path: &str, mode: Option<u32>) -> Option<String> {
    let executable = match mode {
        Some(EXECUTABLE_FILE_MODE) => true,
        Some(REGULAR_FILE_MODE) => false,
        _ => return None,
    };
    let full = root.join(path);
    let changed = full.symlink_metadata().and_then(|meta| {
        // `set_permissions` follows links: only a regular file is changed.
        if !meta.is_file() {
            return Err(io::Error::other("it is not a regular file"));
        }
        let mut permissions = meta.permissions();
        let bits = permissions.mode();
        permissions.set_mode(if executable {
            bits | OWNER_EXECUTE
        } else {
            bits & !OWNER_EXECUTE
        });
        fs::set_permissions(&full, permissions)
    });
    changed
        .err()
        .map(|error| format!("Cannot change the mode of {path}: {error}"))
}

// `watch` runs at each `Moment` of each file: the tests write there as another program would.
fn apply_watching(
    repo: &Repo,
    patch: &Patch,
    mapping: &PathMapping,
    files: &Files,
    watch: &mut dyn FnMut(Moment, &str),
) -> Result<ApplyResult, GitError> {
    let root = repo.path;
    let chosen: BTreeSet<&str> = files.selected.iter().map(String::as_str).collect();
    let overwrite: BTreeSet<&str> = files.overwrite.iter().map(String::as_str).collect();
    let mut failed = Vec::new();
    let mut messages = Vec::new();
    let mut skipped = 0;
    let mut jobs = Vec::new();
    for (index, file) in patch.files.iter().enumerate() {
        let (target, error) = target_of(root, index, file, &mapping.of(index));
        if !chosen.contains(target.path.as_str()) {
            continue;
        }
        let check = match error {
            Some(error) => Check::Fail(error),
            None => precheck(root, &target).map_err(git::io_error)?,
        };
        match check {
            Check::Go => jobs.push(Job { target, file }),
            Check::Exists(_) if target.kind == ChangeKind::Added => {
                if overwrite.contains(target.path.as_str()) {
                    jobs.push(Job { target, file });
                } else {
                    skipped += 1;
                }
            }
            Check::Skip => skipped += 1,
            Check::Exists(message) | Check::Missing(message) | Check::Fail(message) => {
                messages.push(message);
                failed.push(target.path);
            }
        }
    }
    // `PatchApplier.nonWriteActionPreCheck`, as written: the skipped files are compared with the
    // ones left, so a patch whose files are all skipped is PARTIAL.
    let precheck = if !failed.is_empty() {
        ApplyStatus::Failure
    } else if skipped == 0 {
        ApplyStatus::Success
    } else if skipped == jobs.len() {
        ApplyStatus::AlreadyApplied
    } else {
        ApplyStatus::Partial
    };
    // `executePatchGroup`: a successful pre-check says nothing; the files do.
    let mut status = (precheck != ApplyStatus::Success).then_some(precheck);

    let paths: BTreeSet<String> = jobs
        .iter()
        .flat_map(|job| job.target.paths().cloned())
        .collect();
    let paths: Vec<String> = paths.into_iter().collect();
    // Nothing reaches the writer: no copy.
    let snapshot = if paths.is_empty() {
        None
    } else {
        Some(snapshots::create(repo.data, root, &paths, APPLY_REASON).map_err(git::io_error)?)
    };
    let stored = match &snapshot {
        Some(id) => snapshots::stored_files(repo.data, id).map_err(git::io_error)?,
        None => BTreeMap::new(),
    };

    // What was read must be what the copy holds, and what the plan read: anything newer would not
    // be recoverable, or was not what the user chose to patch.
    let mut writer = Writer {
        root,
        snapshot: snapshot.clone().unwrap_or_default(),
        read: BTreeMap::new(),
        watch,
        index: IndexChanges::default(),
        notes: Vec::new(),
    };
    let mut ready = Vec::new();
    for job in jobs {
        let mut same = true;
        for path in job.target.paths() {
            (writer.watch)(Moment::Read, path);
            let now = contents(root, path).ok();
            let copy = stored.get(path).and_then(|file| fs::read(file).ok());
            same &= now == Some(copy);
            if let (Some(now), Some(planned)) = (&now, files.expected.get(path)) {
                same &= hash_of(now.as_deref()) == *planned;
            }
            writer.read.insert(path.clone(), now.flatten());
        }
        if same {
            ready.push(job);
        } else {
            messages.push(changed_while_applying(&job.target.path));
            failed.push(job.target.path);
            status = ApplyStatus::and(status, Some(ApplyStatus::Failure));
        }
    }

    // `applyList`: the texts, then the binaries.
    let mut written = None;
    let mut conflicts = Vec::new();
    ready.sort_by_key(|job| matches!(job.file, FilePatch::Binary(_)));
    for job in &ready {
        let done = match job.file {
            FilePatch::Text(text) => writer.text(job, text),
            FilePatch::Binary(binary) => writer.binary(repo, job, binary),
        };
        match done {
            Done::Applied(applied) => written = ApplyStatus::and(written, Some(applied)),
            Done::Conflict(conflict) => conflicts.push(conflict),
            Done::Failed(message) => {
                messages.extend(message);
                failed.push(job.target.path.clone());
                written = ApplyStatus::and(written, Some(ApplyStatus::Failure));
            }
        }
    }
    // `executeWritable` gives SUCCESS when nothing was done. With conflicts still to answer, the
    // status is theirs: SKIP, which `and` leaves to whatever the caller folds in.
    let nothing = if conflicts.is_empty() {
        ApplyStatus::Success
    } else {
        ApplyStatus::Skip
    };
    let status =
        ApplyStatus::and(status, Some(written.unwrap_or(nothing))).unwrap_or(ApplyStatus::Failure);

    messages.append(&mut writer.notes);
    messages.extend(update_index(root, &writer.index));
    Ok(ApplyResult {
        snapshot,
        status,
        failed,
        conflicts,
        messages,
    })
}

// `git add` for the new files that git does not ignore and `git rm --cached` for the deleted
// ones. A failure leaves the files written: it is reported, not undone.
fn update_index(root: &Path, changes: &IndexChanges) -> Vec<String> {
    let run = |head: &[&str], paths: &[String]| {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        git::git_with_paths(root, true, head, paths)
    };
    let removed = run(
        &["rm", "--cached", "-q", "--ignore-unmatch"],
        &changes.removed,
    );
    let added = ignored(root, &changes.added).and_then(|ignored| {
        let added: Vec<String> = changes
            .added
            .iter()
            .filter(|path| !ignored.contains(*path))
            .cloned()
            .collect();
        run(&["add"], &added)
    });
    [removed, added]
        .into_iter()
        .filter_map(Result::err)
        .map(|error| ApiError::from(error).message)
        .collect()
}

// The paths git ignores, in one `check-ignore` (exit code 1: none). `check-ignore` takes no
// `--literal-pathspecs`, and reads `:(…)` as pathspec magic: each path goes as `./<path>`, which it
// takes literally, and comes back the same way.
fn ignored(root: &Path, paths: &[String]) -> Result<BTreeSet<String>, GitError> {
    if paths.is_empty() {
        return Ok(BTreeSet::new());
    }
    let mut input = Vec::new();
    for path in paths {
        input.extend_from_slice(b"./");
        input.extend_from_slice(path.as_bytes());
        input.push(0);
    }
    let output = match git::run_with_input(root, &["check-ignore", "-z", "--stdin"], &input) {
        Ok(output) => output,
        Err(GitError::Failed { code: Some(1), .. }) => return Ok(BTreeSet::new()),
        Err(error) => return Err(error),
    };
    Ok(output
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8_lossy(path.strip_prefix(b"./").unwrap_or(path)).into_owned())
        .collect())
}

// The hash to expect next, and what went wrong without undoing the write (a mode not set).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolved {
    pub hash: String,
    pub messages: Vec<String>,
}

// Writes the merge window's result over the file, only if it still has `local_hash`, then does
// what the apply left for after it: the mode and the move.
pub fn resolve_conflict(
    repo: &Path,
    path: &str,
    content: &str,
    local_hash: &str,
    moved_to: Option<&str>,
    new_mode: Option<u32>,
) -> Result<Resolved, GitError> {
    resolve_conflict_with(
        repo,
        path,
        content,
        local_hash,
        moved_to,
        new_mode,
        &mut |_| {},
    )
}

// `after_write` runs right after the merge is written: the tests act there as another program.
fn resolve_conflict_with(
    repo: &Path,
    path: &str,
    content: &str,
    local_hash: &str,
    moved_to: Option<&str>,
    new_mode: Option<u32>,
    after_write: &mut dyn FnMut(&Path),
) -> Result<Resolved, GitError> {
    for checked in std::iter::once(path).chain(moved_to) {
        snapshots::check_path(repo, checked).map_err(git::io_error)?;
    }
    if let Some(mode) = new_mode.filter(|mode| !valid_mode(*mode)) {
        return Err(GitError::Io(unknown_mode(path, mode)));
    }
    let changed = || GitError::Io(FILE_CHANGED.to_string());
    let Ok(Some(now)) = contents(repo, path) else {
        return Err(changed());
    };
    if content_hash(&now) != local_hash {
        return Err(changed());
    }
    if let Some(after) = moved_to {
        if kind(repo, after).map_err(git::io_error)? != Kind::Absent {
            return Err(GitError::Io(already_exists(after)));
        }
    }
    let bytes = with_separator(&to_lf(content.as_bytes()), separator(&now));
    let full = repo.join(path);
    let permissions = full.symlink_metadata().ok().map(|meta| meta.permissions());
    storage::write_atomic_with(&full, &bytes, permissions).map_err(git::io_error)?;
    after_write(&full);
    // The merge is written: from here on a failure is told, not returned, so the caller does not
    // retry against a hash that is no longer the file's.
    let mut messages: Vec<String> = set_mode(repo, path, new_mode).into_iter().collect();
    if let Some(after) = moved_to {
        let tracked = tracked(repo, path);
        match move_to(repo, path, after) {
            Ok(()) if tracked => {
                let changes = IndexChanges {
                    added: vec![after.to_string()],
                    removed: vec![path.to_string()],
                };
                messages.extend(update_index(repo, &changes));
            }
            Ok(()) => {}
            Err(error) => messages.push(error),
        }
    }
    Ok(Resolved {
        hash: content_hash(&bytes),
        messages,
    })
}

// Puts back what the copy saved, removes what the apply created and gives the index of those
// files back what it had. Returns the copy `snapshots::restore` takes of the files it puts back;
// the files the apply created are saved, before they are removed, in a copy of their own
// (`before restoring` too).
pub fn rollback(data: &Path, repo: &Path, snapshot: &str) -> Result<SnapshotId, GitError> {
    let copy = snapshots::list(data)
        .into_iter()
        .find(|copy| copy.id == snapshot)
        .ok_or_else(|| GitError::Io(format!("The copy {snapshot} does not exist")))?;
    // What did not exist and does now: saved first, removed after the restore.
    let mut created = Vec::new();
    for entry in &copy.entries {
        snapshots::check_path(repo, &entry.path).map_err(git::io_error)?;
        let now = kind(repo, &entry.path).map_err(git::io_error)?;
        if entry.kind == EntryKind::Absent && matches!(now, Kind::File | Kind::Other) {
            created.push(entry.path.clone());
        }
    }
    if !created.is_empty() {
        snapshots::create(data, repo, &created, snapshots::BEFORE_RESTORE)
            .map_err(git::io_error)?;
    }
    let backup = snapshots::restore(data, repo, snapshot).map_err(git::io_error)?;
    for path in &created {
        let full = repo.join(path);
        fs::remove_file(&full).map_err(git::io_error)?;
        // The folders the apply created, if nothing else is in them.
        let mut folder = full.parent();
        while let Some(dir) = folder.filter(|dir| *dir != repo) {
            if fs::remove_dir(dir).is_err() {
                break;
            }
            folder = dir.parent();
        }
    }
    // The index of what was not staged goes back to HEAD; what was staged as on disk is staged
    // again (what differed from the disk the restore puts back itself).
    let unstaged: Vec<String> = copy
        .entries
        .iter()
        .filter(|entry| !entry.staged)
        .map(|entry| entry.path.clone())
        .collect();
    let staged: Vec<String> = copy
        .entries
        .iter()
        .filter(|entry| entry.staged && !entry.staged_blob && entry.kind != EntryKind::Absent)
        .map(|entry| entry.path.clone())
        .collect();
    // A staged deletion: nothing on disk, nothing in the stage.
    let deleted: Vec<String> = copy
        .entries
        .iter()
        .filter(|entry| entry.staged && !entry.staged_blob && entry.kind == EntryKind::Absent)
        .map(|entry| entry.path.clone())
        .collect();
    if !unstaged.is_empty() {
        git::git_with_paths(repo, true, &["reset", "-q"], &unstaged)?;
    }
    if !staged.is_empty() {
        git::git_with_paths(repo, true, &["add"], &staged)?;
    }
    if !deleted.is_empty() {
        let head = ["rm", "--cached", "-q", "--ignore-unmatch"];
        git::git_with_paths(repo, true, &head, &deleted)?;
    }
    Ok(backup)
}

#[cfg(test)]
mod tests;
