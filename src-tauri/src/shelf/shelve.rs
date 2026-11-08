// Shelve Changes, Shelve Silently and Save To Shelf (`ShelveChangesManager.shelveChanges`,
// `shelveChangesInSeparatedLists`): what a list holds, read once for all the lists, the store, and
// the rollback of only what the shelf stored and was not written meanwhile. The Update saver
// (`preserve::shelve`) reads its lists with the same `lists_contents` and has its own, stricter
// policy for a file written meanwhile: it refuses the whole save.
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::commands::ApiError;
use crate::git::GitError;
use crate::hunks::LineSelection;
use crate::patch::create;
use crate::patch::{patch_failure, PatchSource};
use crate::preserve::shelve::{changed_while_shelving, AsRead};
use crate::shelf::{self, BinaryChange, ShelvedList};
use crate::snapshots::SnapshotId;

pub(crate) const SHELVE_REASON: &str = "Shelve changes";
// rama's own: the changes were read from a repository that is no longer the open one.
pub(crate) const SWITCHED_BEFORE_SHELVING: &str =
    "Another repository was opened before the changes were shelved";

fn shelf_io(error: std::io::Error) -> GitError {
    GitError::Io(error.to_string())
}

// One change to shelve: HEAD against what the list has of the file, the whole file or only its
// chosen hunks and lines (`selection`), as `create::changelist_contents` reads it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShelveEntry {
    pub path: String,
    pub old_path: Option<String>,
    pub selection: Option<Vec<LineSelection>>,
}

// The changes of one changelist for Shelve Silently and Save To Shelf, and the list's name.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShelveGroup {
    pub name: String,
    pub entries: Vec<ShelveEntry>,
}

// `ShelveChangesCommitSession.execute`: only directories (`shelve.changes.only.directories`), or
// the new list, the copy taken before rolling back, what could not be rolled back, and the paths
// the shelf did not store (a folder, a submodule), which were left as they are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Shelved {
    OnlyDirectories,
    Done {
        list: ShelvedList,
        snapshot: Option<SnapshotId>,
        rollback_error: Option<String>,
        skipped: Vec<String>,
    },
}

// A changelist Shelve Silently could not shelve, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ShelveFailure {
    name: String,
    reason: String,
}

// `shelveChangesInSeparatedLists`: the lists made, in the order of the changelists, those that
// failed (`shelve.failed.message`) and the paths no list stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShelvedSilently {
    lists: Vec<ShelvedList>,
    failed: Vec<ShelveFailure>,
    snapshot: Option<SnapshotId>,
    rollback_error: Option<String>,
    skipped: Vec<String>,
}

// `ChangesUtil.getFilePath(change).isDirectory()`: a folder (a submodule) is never shelved.
fn is_directory(repo: &Path, path: &str) -> bool {
    std::fs::symlink_metadata(repo.join(path)).is_ok_and(|meta| meta.is_dir())
}

fn non_directories<'a>(
    repo: &'a Path,
    entries: &'a [ShelveEntry],
) -> impl Iterator<Item = &'a ShelveEntry> {
    entries
        .iter()
        .filter(move |entry| !is_directory(repo, &entry.path))
}

// What one list holds (`createShelfFromChanges`): the text changes as a patch with their bases,
// the binary files, which the store copies whole, and which entries those are. Only `stored` may be
// rolled back; `skipped` are the entries that gave nothing to store (a folder, a submodule).
pub(crate) struct ShelfContents<'a> {
    pub(crate) patch: crate::patch::Patch,
    pub(crate) binaries: Vec<BinaryChange>,
    pub(crate) stored: Vec<&'a ShelveEntry>,
    pub(crate) skipped: Vec<String>,
}

// The contents of several lists, read together: one `Reader` stores the files of every list with
// one `git add` on one copy of the index. Each list fails on its own, with the first error of its
// files, as reading it alone would.
pub(crate) fn lists_contents<'a>(
    repo: &Path,
    groups: &[&'a [ShelveEntry]],
) -> Vec<Result<ShelfContents<'a>, GitError>> {
    let mut read = Vec::new();
    let mut places: Vec<Vec<Option<usize>>> = Vec::new();
    for entries in groups {
        let mut place = Vec::new();
        for entry in entries.iter() {
            if is_directory(repo, &entry.path) {
                place.push(None);
                continue;
            }
            place.push(Some(read.len()));
            read.push(create::changelist_entry(
                &entry.path,
                entry.old_path.as_deref(),
                entry.selection.as_deref(),
            ));
        }
        places.push(place);
    }
    let mut sources: Vec<_> = create::changelist_contents(repo, &read)
        .into_iter()
        .map(Some)
        .collect();
    groups
        .iter()
        .zip(places)
        .map(|(entries, place)| {
            let mut taken = Vec::new();
            for (entry, index) in entries.iter().zip(place) {
                let source = match index {
                    Some(index) => sources[index].take().expect("read once"),
                    None => Ok(None),
                };
                taken.push((entry, source.map_err(patch_failure)?));
            }
            contents_of(repo, taken)
        })
        .collect()
}

pub(crate) fn shelf_contents<'a>(
    repo: &Path,
    entries: &'a [ShelveEntry],
) -> Result<ShelfContents<'a>, GitError> {
    lists_contents(repo, &[entries])
        .pop()
        .expect("one list was read")
}

fn contents_of<'a>(
    repo: &Path,
    read: Vec<(&'a ShelveEntry, Option<PatchSource>)>,
) -> Result<ShelfContents<'a>, GitError> {
    let mut texts = Vec::new();
    let mut binaries = Vec::new();
    let mut stored = Vec::new();
    let mut skipped = Vec::new();
    for (entry, source) in read {
        match source {
            Some(source) if source.binary => binaries.push(BinaryChange {
                before: source.before.map(|(path, _)| path),
                after: source.after.map(|(path, _)| path),
            }),
            Some(source) => texts.push(source),
            None => {
                skipped.push(entry.path.clone());
                continue;
            }
        }
        stored.push(entry);
    }
    let options = crate::patch::WriteOptions {
        include_base: true,
        ..crate::patch::WriteOptions::default()
    };
    let patch = crate::patch::build(repo, &texts, &options).map_err(patch_failure)?;
    Ok(ShelfContents {
        patch,
        binaries,
        stored,
        skipped,
    })
}

// The paths a rollback of these entries can write: each file, and the old name of a rename.
pub(crate) fn touched<'a>(entries: impl IntoIterator<Item = &'a ShelveEntry>) -> Vec<String> {
    let mut paths = std::collections::BTreeSet::new();
    for entry in entries {
        paths.insert(entry.path.clone());
        paths.extend(entry.old_path.clone());
    }
    paths.into_iter().collect()
}

// The chosen lines of one file from several entries (the parts of a file split across lists): a
// hunk taken whole by any of them goes whole, otherwise its lines are joined.
fn joined_selection(parts: &[&[LineSelection]]) -> Vec<LineSelection> {
    let mut joined: Vec<LineSelection> = Vec::new();
    for selection in parts.iter().flat_map(|part| part.iter()) {
        match joined.iter_mut().find(|known| known.hunk == selection.hunk) {
            None => joined.push(selection.clone()),
            Some(known) => match (&mut known.lines, &selection.lines) {
                (Some(lines), Some(more)) => {
                    lines.extend(more);
                    lines.sort_unstable();
                    lines.dedup();
                }
                (lines, _) => *lines = None,
            },
        }
    }
    joined
}

// `rollbackChangesAfterShelve`: `RollbackWorker.doRollback(changes, deleteLocallyAddedFiles=true)`,
// only for the entries the shelf stored. Whole files go back with `rollback_paths`, the shelved
// lines of the others with `rollback_lines`; each takes its own copy first. What fails is
// reported, the rest still goes back.
fn roll_back_shelved<'a>(
    repo: &Path,
    data: &Path,
    stored: impl IntoIterator<Item = &'a ShelveEntry>,
) -> Option<String> {
    let mut whole: Vec<String> = Vec::new();
    let mut parts: Vec<(String, Vec<&[LineSelection]>)> = Vec::new();
    for entry in stored {
        match &entry.selection {
            None => {
                if !whole.contains(&entry.path) {
                    whole.push(entry.path.clone());
                }
            }
            Some(selection) => match parts.iter_mut().find(|(path, _)| *path == entry.path) {
                Some((_, known)) => known.push(selection),
                None => parts.push((entry.path.clone(), vec![selection])),
            },
        }
    }
    let mut errors = Vec::new();
    if !whole.is_empty() {
        if let Err(error) = crate::rollback::rollback_paths(repo, data, &whole, true) {
            errors.push(error.to_string());
        }
    }
    for (path, selections) in parts.iter().filter(|(path, _)| !whole.contains(path)) {
        let selection = joined_selection(selections);
        if let Err(error) = crate::hunks::rollback_lines(repo, data, path, &selection) {
            errors.push(error.to_string());
        }
    }
    (!errors.is_empty()).then(|| errors.join("\n"))
}

// The copy right before the rollback, then only the entries still as they were read go back
// (`preserve::shelve::AsRead`): what was written meanwhile is in neither the shelf nor the copy,
// so it is left on disk and reported with the other rollback errors.
fn roll_back_as_read(
    repo: &Path,
    data: &Path,
    as_read: &AsRead,
    stored: Vec<&ShelveEntry>,
) -> Result<(SnapshotId, Option<String>), GitError> {
    let paths = touched(stored.iter().copied());
    let snapshot = crate::snapshots::create(data, repo, &paths, SHELVE_REASON).map_err(shelf_io)?;
    let changed = as_read.changed(repo, &paths)?;
    let unchanged = stored.into_iter().filter(|entry| {
        let old = entry.old_path.as_ref();
        !changed.contains(&entry.path) && old.is_none_or(|old| !changed.contains(old))
    });
    let mut errors: Vec<String> = roll_back_shelved(repo, data, unchanged)
        .into_iter()
        .collect();
    if !changed.is_empty() {
        errors.push(changed_while_shelving(&changed));
    }
    Ok((snapshot, (!errors.is_empty()).then(|| errors.join("\n"))))
}

// `ShelveChangesManager.shelveChanges`. Everything is read first; nothing is written if another
// repository was opened meanwhile (`still_open`). A failure to store rolls nothing back.
pub(crate) fn shelve(
    repo: &Path,
    data: &Path,
    entries: &[ShelveEntry],
    description: &str,
    rollback: bool,
    still_open: impl Fn() -> bool,
) -> Result<Shelved, GitError> {
    if !entries.is_empty() && non_directories(repo, entries).next().is_none() {
        return Ok(Shelved::OnlyDirectories);
    }
    crate::commit::ensure_idle(repo, data).map_err(|error| GitError::Io(error.to_string()))?;
    // Only a rollback needs what the files were: Save To Shelf reads each file once.
    let as_read = match rollback {
        true => AsRead::take(repo, &touched(entries))?,
        false => AsRead::nothing(),
    };
    let contents = shelf_contents(repo, entries)?;
    // Nothing that is not a folder gave a change to store: no empty list is made.
    if contents.stored.is_empty() {
        return Ok(Shelved::OnlyDirectories);
    }
    if !still_open() {
        return Err(GitError::Io(SWITCHED_BEFORE_SHELVING.to_string()));
    }
    let list = shelf::create(
        data,
        repo,
        description,
        &contents.patch,
        &contents.binaries,
        false,
    )
    .map_err(shelf_io)?;
    let (snapshot, rollback_error) = match rollback {
        true => match roll_back_as_read(repo, data, &as_read, contents.stored.clone()) {
            Ok((snapshot, error)) => (Some(snapshot), error),
            // Nothing was rolled back: the list goes to Recently Deleted.
            Err(error) => {
                let _ = shelf::delete(data, &list.name, std::time::SystemTime::now());
                return Err(error);
            }
        },
        false => (None, None),
    };
    Ok(Shelved::Done {
        list,
        snapshot,
        rollback_error,
        skipped: contents.skipped,
    })
}

fn failure_reason(error: GitError) -> String {
    ApiError::from(error).message
}

// `shelveChangesInSeparatedLists`. A list that cannot be read or stored is reported by name and
// the others go on; only the changes of the lists made are rolled back.
pub(crate) fn shelve_silently(
    repo: &Path,
    data: &Path,
    groups: &[ShelveGroup],
    rollback: bool,
    still_open: impl Fn() -> bool,
) -> Result<ShelvedSilently, GitError> {
    crate::commit::ensure_idle(repo, data).map_err(|error| GitError::Io(error.to_string()))?;
    let as_read = match rollback {
        true => AsRead::take(
            repo,
            &touched(groups.iter().flat_map(|group| &group.entries)),
        )?,
        false => AsRead::nothing(),
    };
    let entries: Vec<&[ShelveEntry]> = groups.iter().map(|group| &group.entries[..]).collect();
    let read = lists_contents(repo, &entries);
    if !still_open() {
        return Err(GitError::Io(SWITCHED_BEFORE_SHELVING.to_string()));
    }
    let mut lists = Vec::new();
    let mut failed = Vec::new();
    let mut skipped = Vec::new();
    let mut shelved: Vec<&ShelveEntry> = Vec::new();
    for (group, contents) in groups.iter().zip(read) {
        // A changelist that gave nothing to store makes no list.
        if let Ok(contents) = &contents {
            if contents.stored.is_empty() {
                skipped.extend(contents.skipped.iter().cloned());
                continue;
            }
        }
        let made = contents.and_then(|contents| {
            shelf::create(
                data,
                repo,
                &group.name,
                &contents.patch,
                &contents.binaries,
                false,
            )
            .map(|list| (list, contents))
            .map_err(shelf_io)
        });
        match made {
            Ok((list, contents)) => {
                lists.push(list);
                shelved.extend(contents.stored);
                skipped.extend(contents.skipped);
            }
            // `LOG.warn(e)` of `shelveChangesInSeparatedLists`; the reason also goes to the user.
            Err(error) => {
                let reason = failure_reason(error);
                eprintln!("Shelving changelist \"{}\" failed: {reason}", group.name);
                failed.push(ShelveFailure {
                    name: group.name.clone(),
                    reason,
                });
            }
        }
    }
    let (snapshot, rollback_error) = match rollback && !shelved.is_empty() {
        true => match roll_back_as_read(repo, data, &as_read, shelved) {
            Ok((snapshot, error)) => (Some(snapshot), error),
            // Nothing was rolled back: the lists go to Recently Deleted.
            Err(error) => {
                for list in &lists {
                    let _ = shelf::delete(data, &list.name, std::time::SystemTime::now());
                }
                return Err(error);
            }
        },
        false => (None, None),
    };
    Ok(ShelvedSilently {
        lists,
        failed,
        snapshot,
        rollback_error,
        skipped,
    })
}

// The repository has a file with two hunks, a binary, a staged rename and two other files: what
// the tests shelve is checked on disk, in the store and in the snapshots.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::hunks::{self, LineSelection};
    use crate::shelf;
    use crate::snapshots;
    use crate::test_support::TempRepo;
    use std::fs;
    use tempfile::TempDir;

    const LINES: &str = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n";
    const EDITED: &str = "one\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\ntwelve\n";
    const PNG: [u8; 6] = [0, 159, 146, 150, 0, 1];

    fn repo() -> TempRepo {
        let repo = TempRepo::new();
        for (path, bytes) in [
            ("a.txt", LINES.as_bytes()),
            ("b.txt", b"b\n"),
            ("c.txt", b"c\n"),
            ("old.txt", b"old\n"),
            ("img.png", &PNG),
        ] {
            fs::write(repo.path().join(path), bytes).unwrap();
        }
        repo.git(&["add", "-A"]);
        repo.git(&["commit", "-q", "-m", "base"]);
        fs::write(repo.path().join("a.txt"), EDITED).unwrap();
        fs::write(repo.path().join("b.txt"), b"B\n").unwrap();
        fs::write(repo.path().join("c.txt"), b"C\n").unwrap();
        fs::write(repo.path().join("img.png"), [0, 159, 146, 150, 0, 2]).unwrap();
        repo.git(&["mv", "old.txt", "new.txt"]);
        repo
    }

    fn read(repo: &TempRepo, path: &str) -> String {
        fs::read_to_string(repo.path().join(path)).unwrap()
    }

    fn whole(path: &str) -> ShelveEntry {
        ShelveEntry {
            path: path.to_string(),
            old_path: None,
            selection: None,
        }
    }

    // The first hunk of the file, whole.
    fn first_hunk(repo: &TempRepo, path: &str) -> ShelveEntry {
        let hunks = hunks::file_hunks(repo.path(), path).unwrap();
        assert_eq!(hunks.len(), 2);
        ShelveEntry {
            selection: Some(vec![LineSelection {
                hunk: hunks[0].id.clone(),
                lines: None,
            }]),
            ..whole(path)
        }
    }

    fn patch_text(data: &Path, list: &ShelvedList) -> String {
        let patch = shelf::load_patch(data, &list.name).unwrap();
        let options = crate::patch::WriteOptions::default();
        String::from_utf8(crate::patch::write(&patch, &options)).unwrap()
    }

    fn done(shelved: Shelved) -> (ShelvedList, Option<SnapshotId>, Option<String>) {
        match shelved {
            Shelved::Done {
                list,
                snapshot,
                rollback_error,
                ..
            } => (list, snapshot, rollback_error),
            Shelved::OnlyDirectories => panic!("only directories"),
        }
    }

    // `ShelveChangesCommitSession.execute`: `shelveChanges(…, rollback=true)`. The included part
    // goes to the shelf and is rolled back; the lines of the file that were not chosen stay on
    // disk, and so does what was not included.
    #[test]
    fn shelve_changes_opens_the_dialog_titled_shelve_changes_and_shelves_the_included_part() {
        let repo = repo();
        let data = TempDir::new().unwrap();
        let entries = [
            first_hunk(&repo, "a.txt"),
            whole("b.txt"),
            whole("img.png"),
            ShelveEntry {
                old_path: Some("old.txt".to_string()),
                ..whole("new.txt")
            },
        ];

        let shelved = shelve(repo.path(), data.path(), &entries, "Fix\nit", true, || true);

        let (list, snapshot, rollback_error) = done(shelved.unwrap());
        assert_eq!(rollback_error, None);
        assert_eq!(list.description, "Fix it");
        assert_eq!(shelf::list(data.path()), std::slice::from_ref(&list));
        let text = patch_text(data.path(), &list);
        assert!(text.contains("-1\n+one\n"), "{text}");
        assert!(!text.contains("twelve"), "{text}");
        assert!(text.contains("-b\n+B\n"), "{text}");
        assert!(
            text.contains("rename from old.txt\nrename to new.txt"),
            "{text}"
        );
        assert!(!text.contains("c.txt"), "{text}");
        assert!(!text.contains("img.png"), "{text}");
        let copy = list.binaries[0].shelved.clone().unwrap();
        assert_eq!(fs::read(copy).unwrap(), [0, 159, 146, 150, 0, 2]);
        assert_eq!(list.binaries[0].before.as_deref(), Some("img.png"));

        // Rolled back: the chosen hunk, the whole files and the rename. Kept: the other hunk and
        // what was not included.
        assert_eq!(read(&repo, "a.txt"), LINES.replace("12\n", "twelve\n"));
        assert_eq!(read(&repo, "b.txt"), "b\n");
        assert_eq!(fs::read(repo.path().join("img.png")).unwrap(), PNG);
        assert_eq!(read(&repo, "old.txt"), "old\n");
        assert!(!repo.path().join("new.txt").exists());
        assert_eq!(read(&repo, "c.txt"), "C\n");
        // The copy taken before anything was written has every path, the old name included.
        let snapshot = snapshot.unwrap();
        let taken = snapshots::list(data.path());
        let first = taken.iter().find(|taken| taken.id == snapshot).unwrap();
        let mut paths: Vec<&str> = first
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect();
        paths.sort();
        assert_eq!(paths, ["a.txt", "b.txt", "img.png", "new.txt", "old.txt"]);
    }

    // `ChangesUtil.hasFileChanges`: with only directories nothing is shelved, rolled back or
    // copied. A directory among files is left out.
    #[test]
    fn only_directories_says_they_cannot_be_shelved() {
        let repo = repo();
        fs::create_dir(repo.path().join("folder")).unwrap();
        let data = TempDir::new().unwrap();

        let shelved = shelve(
            repo.path(),
            data.path(),
            &[whole("folder")],
            "Dirs",
            true,
            || true,
        );

        assert_eq!(shelved.unwrap(), Shelved::OnlyDirectories);
        assert!(shelf::list(data.path()).is_empty());
        assert!(snapshots::list(data.path()).is_empty());

        let mixed = [whole("folder"), whole("b.txt")];
        let (list, _, _) =
            done(shelve(repo.path(), data.path(), &mixed, "Mixed", true, || true).unwrap());
        assert!(!patch_text(data.path(), &list).contains("folder"));
        assert_eq!(read(&repo, "b.txt"), "b\n");
    }

    // The store cannot be written: the copy was taken, and nothing is rolled back.
    #[test]
    fn a_failed_shelve_rolls_nothing_back() {
        let repo = repo();
        let data = TempDir::new().unwrap();
        fs::write(data.path().join("shelf"), b"not a folder").unwrap();

        let shelved = shelve(
            repo.path(),
            data.path(),
            &[whole("b.txt")],
            "Fails",
            true,
            || true,
        );

        assert!(shelved.is_err());
        assert_eq!(read(&repo, "b.txt"), "B\n");
        // The copy comes right before the rollback, which never came.
        assert_eq!(snapshots::list(data.path()).len(), 0);
    }

    // Review Focus 1: a file written after the changes were read is in neither the shelf nor
    // the copy. It is left on disk and reported; the rest goes back.
    #[test]
    fn a_file_written_between_shelving_and_rollback_survives() {
        let repo = repo();
        let data = TempDir::new().unwrap();
        let target = repo.path().join("b.txt");
        let entries = [whole("b.txt"), whole("c.txt")];

        let shelved = shelve(repo.path(), data.path(), &entries, "Late", true, || {
            fs::write(&target, b"devin\n").unwrap();
            true
        });

        let (_, snapshot, error) = done(shelved.unwrap());
        assert_eq!(read(&repo, "b.txt"), "devin\n");
        assert_eq!(read(&repo, "c.txt"), "c\n");
        assert_eq!(
            error.as_deref(),
            Some("\"b.txt\" changed while it was being shelved and was left as it is.")
        );
        let copy = snapshot.unwrap();
        let stored = snapshots::stored(data.path(), &copy, "b.txt", false).unwrap();
        assert_eq!(fs::read_to_string(stored).unwrap(), "devin\n");

        let groups = [ShelveGroup {
            name: "Changes".to_string(),
            entries: vec![whole("a.txt")],
        }];
        let target = repo.path().join("a.txt");
        let silently = shelve_silently(repo.path(), data.path(), &groups, true, || {
            fs::write(&target, b"devin\n").unwrap();
            true
        })
        .unwrap();
        assert_eq!(read(&repo, "a.txt"), "devin\n");
        assert!(silently
            .rollback_error
            .unwrap()
            .starts_with("\"a.txt\" changed"));
    }

    // Review Focus 5: another repository was opened while the task waited for its turn or read
    // the changes. Nothing is written: no list, no copy, no rollback.
    #[test]
    fn a_repository_switch_during_shelve_is_refused_before_writing() {
        let repo = repo();
        let data = TempDir::new().unwrap();

        let shelved = shelve(
            repo.path(),
            data.path(),
            &[whole("b.txt")],
            "Late",
            true,
            || false,
        );
        let silently = shelve_silently(
            repo.path(),
            data.path(),
            &[ShelveGroup {
                name: "Changes".to_string(),
                entries: vec![whole("c.txt")],
            }],
            true,
            || false,
        );

        for error in [shelved.unwrap_err(), silently.unwrap_err()] {
            assert_eq!(error, GitError::Io(SWITCHED_BEFORE_SHELVING.to_string()));
        }
        assert!(!data.path().join("shelf").exists());
        assert!(snapshots::list(data.path()).is_empty());
        assert_eq!(read(&repo, "b.txt"), "B\n");
        assert_eq!(read(&repo, "c.txt"), "C\n");
    }

    // `shelveChangesInSeparatedLists`: one list per changelist, named after it, in the lists'
    // order. A file split across the two lists goes half to each and is rolled back whole. A list
    // that fails is reported by name and the others are still shelved and rolled back.
    #[test]
    fn shelve_silently_makes_one_shelf_per_list_and_rolls_back() {
        let repo = repo();
        let data = TempDir::new().unwrap();
        let hunks = hunks::file_hunks(repo.path(), "a.txt").unwrap();
        let part = |index: usize| ShelveEntry {
            selection: Some(vec![LineSelection {
                hunk: hunks[index].id.clone(),
                lines: None,
            }]),
            ..whole("a.txt")
        };
        let groups = [
            ShelveGroup {
                name: "Changes".to_string(),
                entries: vec![part(0), whole("b.txt")],
            },
            ShelveGroup {
                name: "Broken".to_string(),
                entries: vec![whole("../outside.txt")],
            },
            ShelveGroup {
                name: "Other".to_string(),
                entries: vec![part(1), whole("c.txt")],
            },
        ];

        let shelved = shelve_silently(repo.path(), data.path(), &groups, true, || true).unwrap();

        let names: Vec<&str> = shelved
            .lists
            .iter()
            .map(|list| list.description.as_str())
            .collect();
        assert_eq!(names, ["Changes", "Other"]);
        // The reason is kept for the notification (`LOG.warn(e)` in IntelliJ).
        let failed: Vec<(&str, &str)> = shelved
            .failed
            .iter()
            .map(|failure| (failure.name.as_str(), failure.reason.as_str()))
            .collect();
        assert_eq!(
            failed,
            [(
                "Broken",
                "The path \"../outside.txt\" is not valid inside the repository"
            )]
        );
        assert!(shelved.skipped.is_empty());
        assert_eq!(shelved.rollback_error, None);
        assert!(shelved.snapshot.is_some());
        let changes = patch_text(data.path(), &shelved.lists[0]);
        assert!(
            changes.contains("+one\n") && !changes.contains("twelve") && changes.contains("+B\n")
        );
        let other = patch_text(data.path(), &shelved.lists[1]);
        assert!(other.contains("+twelve\n") && !other.contains("+one") && other.contains("+C\n"));
        assert_eq!(read(&repo, "a.txt"), LINES);
        assert_eq!(read(&repo, "b.txt"), "b\n");
        assert_eq!(read(&repo, "c.txt"), "c\n");
    }

    // Never lose work: what the shelf did not store is not rolled back. A submodule whose folder
    // is gone gives no change to store (`changelist_contents` gives nothing for a gitlink), so
    // it stays as it is.
    #[test]
    fn a_change_the_shelf_did_not_store_is_not_rolled_back() {
        let repo = repo();
        let head = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
        let gitlink = format!("160000,{head},sub");
        repo.git(&["update-index", "--add", "--cacheinfo", &gitlink]);
        repo.git(&["commit", "-q", "-m", "submodule"]);
        let before = repo.git(&["status", "--porcelain", "--", "sub"]);
        assert_eq!(before.trim(), "D sub");
        let data = TempDir::new().unwrap();

        let entries = [whole("sub"), whole("b.txt")];
        let shelved = shelve(repo.path(), data.path(), &entries, "Sub", true, || true).unwrap();

        let Shelved::Done { skipped, .. } = &shelved else {
            panic!("only directories")
        };
        assert_eq!(skipped, &["sub"]);
        let (list, _, rollback_error) = done(shelved);
        assert_eq!(rollback_error, None);
        assert!(!patch_text(data.path(), &list).contains("sub"));
        assert_eq!(repo.git(&["status", "--porcelain", "--", "sub"]), before);
        assert_eq!(read(&repo, "b.txt"), "b\n");
    }

    // `shelve.changes.only.directories` also when nothing that was not a folder gave a change to
    // store (a submodule whose folder is gone): no empty list is made, in either command.
    #[test]
    fn a_shelve_that_stores_nothing_makes_no_list() {
        let repo = repo();
        let head = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
        let gitlink = format!("160000,{head},sub");
        repo.git(&["update-index", "--add", "--cacheinfo", &gitlink]);
        repo.git(&["commit", "-q", "-m", "submodule"]);
        let data = TempDir::new().unwrap();

        let shelved = shelve(
            repo.path(),
            data.path(),
            &[whole("sub")],
            "Sub",
            true,
            || true,
        );
        let groups = [ShelveGroup {
            name: "Changes".to_string(),
            entries: vec![whole("sub")],
        }];
        let silently = shelve_silently(repo.path(), data.path(), &groups, true, || true).unwrap();

        assert_eq!(shelved.unwrap(), Shelved::OnlyDirectories);
        assert!(silently.lists.is_empty());
        assert!(silently.failed.is_empty());
        assert_eq!(silently.skipped, ["sub"]);
        assert_eq!(silently.snapshot, None);
        assert!(shelf::list(data.path()).is_empty());
        assert!(snapshots::list(data.path()).is_empty());
    }

    // An unversioned file that reaches the shelf goes in as a new file, with its content, before
    // the rollback (`delete_added = true`) takes it away.
    #[test]
    fn an_included_unversioned_file_is_stored_before_the_rollback() {
        let repo = repo();
        fs::write(repo.path().join("loose.txt"), b"loose\n").unwrap();
        let data = TempDir::new().unwrap();

        let shelved = shelve(
            repo.path(),
            data.path(),
            &[whole("loose.txt")],
            "Loose",
            true,
            || true,
        );

        let (list, _, rollback_error) = done(shelved.unwrap());
        let text = patch_text(data.path(), &list);
        assert!(text.contains("new file mode"), "{text}");
        assert!(text.contains("+loose\n"), "{text}");
        assert_eq!(rollback_error, None);
        assert!(!repo.path().join("loose.txt").exists());
    }

    // `SaveToShelveAction` (`rollbackChanges = false`): the lists are made, the files stay.
    #[test]
    fn save_to_shelf_does_not_roll_back() {
        let repo = repo();
        let data = TempDir::new().unwrap();
        let groups = [ShelveGroup {
            name: "Changes".to_string(),
            entries: vec![whole("b.txt"), whole("c.txt")],
        }];

        let shelved = shelve_silently(repo.path(), data.path(), &groups, false, || true).unwrap();

        assert_eq!(shelved.lists.len(), 1);
        assert!(shelved.failed.is_empty());
        assert_eq!(shelved.snapshot, None);
        assert_eq!(read(&repo, "b.txt"), "B\n");
        assert_eq!(read(&repo, "c.txt"), "C\n");
        assert!(snapshots::list(data.path()).is_empty());
    }
}
