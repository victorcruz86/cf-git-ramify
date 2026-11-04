use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

use crate::commands::ApiError;
use crate::git::{self, GitError};
use crate::snapshots::{self, SnapshotId};
use crate::status::{self, FileChange, FileState};

const ROLLBACK_REASON: &str = "Rollback";

#[derive(Debug)]
pub enum RollbackError {
    NothingToRollback,
    // Paths in conflict: they have to be resolved first. Nothing is touched and no snapshot is taken.
    Unmerged(Vec<String>),
    // Paths with changes that HEAD does not know (or knows under another name): nothing is touched.
    NotInHead(Vec<String>),
    OutsideRepository(String),
    // A folder on the path's way is now a file or a link: rolling back would replace it
    // with a folder. Nothing is touched and no snapshot is taken.
    Displaced(String),
    // Failed before saving the snapshot: nothing has been touched.
    Failed(String),
    // The snapshot could not be saved: nothing has been touched.
    Snapshot(io::Error),
    // Failed after saving the snapshot: the previous work is in it.
    Interrupted {
        message: String,
        snapshot: SnapshotId,
    },
}

impl fmt::Display for RollbackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NothingToRollback => write!(f, "There are no changes to roll back"),
            Self::Unmerged(paths) => write!(
                f,
                "There are unresolved conflicts ({}). Resolve them before rolling back; nothing has been touched",
                paths.join(", ")
            ),
            Self::NotInHead(paths) => write!(
                f,
                "Not found in the last commit ({}), so it cannot be rolled back; nothing has been touched",
                paths.join(", ")
            ),
            Self::OutsideRepository(path) => {
                write!(f, "The path \"{path}\" is not inside the repository")
            }
            Self::Displaced(path) => write!(
                f,
                "On the way to \"{path}\" there is now a file or a link instead of a folder: rolling it back would replace it, so nothing has been touched"
            ),
            Self::Failed(message) => write!(f, "{message}"),
            Self::Snapshot(error) => write!(
                f,
                "Could not save the backup copy and nothing was touched: {error}"
            ),
            Self::Interrupted { message, snapshot } => write!(
                f,
                "The rollback was interrupted: {message}. Your previous work is in snapshot {snapshot}"
            ),
        }
    }
}

impl std::error::Error for RollbackError {}

fn git_message(error: GitError) -> String {
    ApiError::from(error).message
}

// Without commits there is no HEAD, and then no file exists in it.
fn paths_in_head(repo: &Path, paths: &[String]) -> Result<BTreeSet<String>, GitError> {
    if !git::head_exists(repo)? {
        return Ok(BTreeSet::new());
    }
    let head = ["ls-tree", "-r", "-z", "--name-only", "HEAD"];
    let output = git::git_with_paths(repo, false, &head, paths)?;
    Ok(output
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
        .map(|name| String::from_utf8_lossy(name).into_owned())
        .collect())
}

#[derive(Default)]
struct Plan {
    // They exist in HEAD: they are left again as they are there.
    restore: Vec<String>,
    // They do not exist in HEAD: they leave the index.
    unstage: Vec<String>,
    delete: Vec<Deletion>,
}

struct Deletion {
    path: String,
    // New name of a rename: it is only deleted if the previous name was already restored and is
    // another file. With a volume that does not distinguish case, `a.txt` and `A.txt` are the same.
    restored_from: Option<String>,
}

// Paths that the rollback needs in HEAD and that HEAD does not have. A modified or deleted file absent
// from HEAD (for example, because of another Unicode normalization of the name) would be taken for new and
// deleted; a rename without a source in HEAD has nothing to restore.
fn missing_in_head(changes: &[&FileChange], in_head: &BTreeSet<String>) -> Vec<String> {
    let mut missing = BTreeSet::new();
    for change in changes {
        let required = match change.state {
            FileState::Modified | FileState::Deleted => Some(&change.path),
            FileState::Renamed => Some(change.old_path.as_ref().unwrap_or(&change.path)),
            _ => None,
        };
        if let Some(path) = required.filter(|path| !in_head.contains(*path)) {
            missing.insert(path.clone());
        }
    }
    missing.into_iter().collect()
}

// Decides by what is in HEAD and not only by the state. Only new files (`Added`, `Untracked`)
// are deleted from the disk, with `delete_added`, and the new name of a rename.
fn plan(changes: &[&FileChange], in_head: &BTreeSet<String>, delete_added: bool) -> Plan {
    let mut plan = Plan::default();
    for change in changes {
        let old = match (change.state, &change.old_path) {
            (FileState::Renamed, Some(old)) => Some(old),
            _ => None,
        };
        if let Some(old) = old {
            plan.restore.push(old.clone());
        }
        if in_head.contains(&change.path) {
            plan.restore.push(change.path.clone());
            continue;
        }
        plan.unstage.push(change.path.clone());
        let new_file = matches!(change.state, FileState::Added | FileState::Untracked);
        if old.is_some() || (delete_added && new_file) {
            plan.delete.push(Deletion {
                path: change.path.clone(),
                restored_from: old.cloned(),
            });
        }
    }
    plan
}

type FileId = (u64, u64);

#[cfg(unix)]
fn file_id(meta: &fs::Metadata) -> Result<FileId, String> {
    use std::os::unix::fs::MetadataExt;
    Ok((meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
fn file_id(_meta: &fs::Metadata) -> Result<FileId, String> {
    Err("File identity cannot be checked on this system".to_string())
}

fn git_names(repo: &Path, args: &[&str]) -> Result<Vec<String>, String> {
    let output = git::run(repo, args).map_err(git_message)?;
    Ok(output
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
        .map(|name| String::from_utf8_lossy(name).into_owned())
        .collect())
}

// On-disk identity (device and inode) of everything versioned (index and HEAD) or restored.
// No attempt is made to guess which names match according to the volume's rules (case,
// Unicode): it looks at which files are the same. A file that was going to be deleted and is one of
// these is the one the rollback has just left right, or a hard link of it: it is not deleted.
// It is only computed if there is something to delete.
fn tracked_ids(repo: &Path, plan: &Plan) -> Result<BTreeSet<FileId>, String> {
    let mut names = git_names(repo, &["ls-files", "-z"])?;
    if git::head_exists(repo).map_err(git_message)? {
        names.extend(git_names(
            repo,
            &["ls-tree", "-r", "-z", "--name-only", "HEAD"],
        )?);
    }
    names.extend(plan.restore.iter().cloned());
    let mut ids = BTreeSet::new();
    for name in names {
        if let Some(meta) = on_disk(repo, &name)? {
            ids.insert(file_id(&meta)?);
        }
    }
    Ok(ids)
}

// `None` if the name does not resolve to anything: it does not exist, or some folder on its way is no longer
// a folder.
fn on_disk(repo: &Path, name: &str) -> Result<Option<fs::Metadata>, String> {
    match repo.join(name).symlink_metadata() {
        Ok(meta) => Ok(Some(meta)),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(format!("Could not read \"{name}\": {error}")),
    }
}

// Decides what is deleted before deleting anything, so that a read failure does not leave the deletion
// half done.
fn removable<'a>(repo: &Path, plan: &'a Plan) -> Result<Vec<&'a str>, String> {
    if plan.delete.is_empty() {
        return Ok(Vec::new());
    }
    let tracked = tracked_ids(repo, plan)?;
    let mut removable = Vec::new();
    for deletion in &plan.delete {
        let Some(meta) = on_disk(repo, &deletion.path)? else {
            continue;
        };
        // The new name of a rename is only removed if the previous one is already back.
        if let Some(old) = &deletion.restored_from {
            if let Err(error) = repo.join(old).symlink_metadata() {
                return Err(format!(
                    "\"{old}\" was not restored, so \"{}\" is not deleted: {error}",
                    deletion.path
                ));
            }
        }
        if !tracked.contains(&file_id(&meta)?) {
            removable.push(deletion.path.as_str());
        }
    }
    Ok(removable)
}

// Moments of `apply` in which the disk is checked against the snapshot.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Start,
    Checkout,
    Delete,
}

impl Step {
    fn left(self) -> &'static str {
        match self {
            Self::Start => "nothing has been touched",
            Self::Checkout => "the files remain as they were, but they have left the stage",
            Self::Delete => {
                "everything else has already been rolled back, but nothing has been deleted"
            }
        }
    }
}

// Overwriting or deleting what was written after the snapshot would lose it irrecoverably. What someone writes
// after this check, in the instant before `git`, is still not covered.
fn check_unchanged(
    repo: &Path,
    dir: &Path,
    snapshot: &str,
    step: Step,
    paths: &[String],
) -> Result<(), String> {
    let changed = snapshots::changed_since(dir, repo, snapshot, paths)
        .map_err(|error| format!("Could not compare with the snapshot: {error}"))?;
    if changed.is_empty() {
        return Ok(());
    }
    let names: Vec<String> = changed.iter().map(|path| format!("\"{path}\"")).collect();
    Err(format!(
        "{} changed after the snapshot was saved and is still on disk; {}",
        names.join(", "),
        step.left()
    ))
}

fn apply(
    repo: &Path,
    plan: &Plan,
    check: &impl Fn(Step, &[String]) -> Result<(), String>,
) -> Result<(), String> {
    let at_risk: Vec<String> = plan
        .restore
        .iter()
        .cloned()
        .chain(plan.delete.iter().map(|deletion| deletion.path.clone()))
        .collect();
    check(Step::Start, &at_risk)?;
    let run = |head: &[&str], paths: &[String]| {
        if paths.is_empty() {
            return Ok(());
        }
        git::git_with_paths(repo, true, head, paths)
            .map(drop)
            .map_err(git_message)
    };
    // New names first: with a volume that does not distinguish case, removing `A.txt`
    // after restoring `a.txt` could also take away the restored entry.
    run(
        &["rm", "--cached", "-q", "-f", "--ignore-unmatch"],
        &plan.unstage,
    )?;
    run(&["reset", "-q", "HEAD"], &plan.restore)?;
    check(Step::Checkout, &at_risk)?;
    run(&["checkout", "-q", "HEAD"], &plan.restore)?;
    let removable = removable(repo, plan)?;
    let names: Vec<String> = removable.iter().map(|path| path.to_string()).collect();
    check(Step::Delete, &names)?;
    for path in removable {
        match fs::remove_file(repo.join(path)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Could not delete \"{path}\": {error}")),
        }
    }
    Ok(())
}

// Safety order: reject conflicts, validate paths, check HEAD, save the snapshot and
// only then touch the repository. New files leave the index and are only deleted from the
// disk with `delete_added`; a rename goes back to the previous name and removes the new one.
pub fn rollback(
    repo: &Path,
    dir: &Path,
    changes: &[FileChange],
    delete_added: bool,
) -> Result<SnapshotId, RollbackError> {
    rollback_pausing(repo, dir, changes, delete_added, |_| {})
}

// `pause` is the tests' seam: what Devin writes right at that moment.
fn rollback_pausing(
    repo: &Path,
    dir: &Path,
    changes: &[FileChange],
    delete_added: bool,
    pause: impl Fn(Step),
) -> Result<SnapshotId, RollbackError> {
    let changes: Vec<&FileChange> = changes
        .iter()
        .filter(|change| change.state != FileState::Ignored)
        .collect();
    if changes.is_empty() {
        return Err(RollbackError::NothingToRollback);
    }
    // `reset` would delete the conflict stages and consider it resolved with HEAD's version,
    // leaving `MERGE_HEAD`: the next commit would discard the other side.
    let unmerged: BTreeSet<String> = changes
        .iter()
        .filter(|change| change.state == FileState::Unmerged)
        .map(|change| change.path.clone())
        .collect();
    if !unmerged.is_empty() {
        return Err(RollbackError::Unmerged(unmerged.into_iter().collect()));
    }

    let mut targets = BTreeSet::new();
    for change in &changes {
        targets.insert(change.path.clone());
        if let (FileState::Renamed, Some(old)) = (change.state, &change.old_path) {
            targets.insert(old.clone());
        }
    }
    for path in &targets {
        snapshots::check_path(repo, path).map_err(|error| match error.kind() {
            io::ErrorKind::InvalidInput => RollbackError::OutsideRepository(path.clone()),
            _ => RollbackError::Snapshot(error),
        })?;
    }

    // Restoring `d/f` with `d` turned into a link or file would delete `d` and replace it with a folder.
    if let Some(path) = targets
        .iter()
        .find(|path| snapshots::behind_other(repo, path))
    {
        return Err(RollbackError::Displaced(path.clone()));
    }

    let targets: Vec<String> = targets.into_iter().collect();
    let in_head =
        paths_in_head(repo, &targets).map_err(|error| RollbackError::Failed(git_message(error)))?;
    let missing = missing_in_head(&changes, &in_head);
    if !missing.is_empty() {
        return Err(RollbackError::NotInHead(missing));
    }
    let plan = plan(&changes, &in_head, delete_added);
    // The guard is repeated after restoring, but if the versioned files cannot be read it is better
    // to know before touching anything than to leave a rename half done.
    if !plan.delete.is_empty() {
        tracked_ids(repo, &plan).map_err(RollbackError::Failed)?;
    }

    let snapshot =
        snapshots::create(dir, repo, &targets, ROLLBACK_REASON).map_err(RollbackError::Snapshot)?;
    let check = |step, paths: &[String]| {
        pause(step);
        check_unchanged(repo, dir, &snapshot, step, paths)
    };
    apply(repo, &plan, &check).map_err(|message| RollbackError::Interrupted {
        message,
        snapshot: snapshot.clone(),
    })?;
    Ok(snapshot)
}

// For the interface: takes the current state and rolls back only the requested paths that still have changes.
pub fn rollback_paths(
    repo: &Path,
    dir: &Path,
    paths: &[String],
    delete_added: bool,
) -> Result<SnapshotId, RollbackError> {
    let wanted: BTreeSet<&String> = paths.iter().collect();
    let changes: Vec<FileChange> = status::read(repo, false)
        .map_err(|error| RollbackError::Failed(git_message(error)))?
        .into_iter()
        .filter(|change| wanted.contains(&change.path))
        .collect();
    rollback(repo, dir, &changes, delete_added)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshots::{self, EntryKind};
    use crate::status::{self, FileState};
    use crate::test_support::{TempRepo, DAMAGED_REFS};
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn write(repo: &TempRepo, file: &str, contents: &[u8]) {
        let target = repo.path().join(file);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, contents).unwrap();
    }

    fn read(repo: &TempRepo, file: &str) -> Vec<u8> {
        fs::read(repo.path().join(file)).unwrap()
    }

    fn exists(repo: &TempRepo, file: &str) -> bool {
        repo.path().join(file).symlink_metadata().is_ok()
    }

    fn changes(repo: &TempRepo) -> Vec<FileChange> {
        status::read(repo.path(), false).unwrap()
    }

    fn copy_of(dir: &TempDir, id: &str, file: &str) -> PathBuf {
        snapshots::stored(dir.path(), id, file, false).unwrap()
    }

    fn paths(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn index(repo: &TempRepo) -> String {
        repo.git(&["ls-files", "-s"])
    }

    fn assert_interrupted(result: Result<SnapshotId, RollbackError>, dir: &TempDir, path: &str) {
        let Err(RollbackError::Interrupted { message, snapshot }) = result else {
            panic!("{result:?}")
        };
        assert!(message.contains(path), "{message}");
        assert!(copy_of(dir, &snapshot, path).exists());
    }

    #[test]
    fn a_change_after_the_snapshot_stops_the_rollback_without_touching_anything() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1", "one");
        repo.commit("b.txt", b"1", "two");
        write(&repo, "a.txt", b"2");
        write(&repo, "b.txt", b"2");
        repo.git(&["add", "a.txt"]);
        write(&repo, "a.txt", b"3");
        let entries = index(&repo);
        let devin = |step| {
            if step == Step::Start {
                write(&repo, "a.txt", b"written by Devin, with another length");
            }
        };

        let result = rollback_pausing(repo.path(), dir.path(), &changes(&repo), false, devin);

        assert_interrupted(result, &dir, "a.txt");
        assert_eq!(
            read(&repo, "a.txt"),
            b"written by Devin, with another length"
        );
        assert_eq!(read(&repo, "b.txt"), b"2");
        assert_eq!(index(&repo), entries);
    }

    // Same size: only the date gives it away.
    #[test]
    fn a_change_right_before_the_checkout_is_not_overwritten() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1", "one");
        repo.commit("b.txt", b"1", "two");
        write(&repo, "a.txt", b"2");
        write(&repo, "b.txt", b"2");
        let devin = |step| {
            if step == Step::Checkout {
                write(&repo, "a.txt", b"3");
            }
        };

        let result = rollback_pausing(repo.path(), dir.path(), &changes(&repo), false, devin);

        assert_interrupted(result, &dir, "a.txt");
        assert_eq!(read(&repo, "a.txt"), b"3");
        assert_eq!(read(&repo, "b.txt"), b"2");
    }

    #[test]
    fn a_new_file_that_changes_before_being_deleted_is_not_deleted() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1", "one");
        write(&repo, "new.txt", b"new");
        repo.git(&["add", "new.txt"]);
        let devin = |step| {
            if step == Step::Delete {
                write(&repo, "new.txt", b"written by Devin");
            }
        };

        let result = rollback_pausing(repo.path(), dir.path(), &changes(&repo), true, devin);

        assert_interrupted(result, &dir, "new.txt");
        assert_eq!(read(&repo, "new.txt"), b"written by Devin");
    }

    // It is checked before removing anything from the stage: a rename is not left half done.
    #[test]
    fn a_new_file_that_changes_after_the_snapshot_stops_the_rollback_from_the_start() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1", "one");
        write(&repo, "new.txt", b"new");
        repo.git(&["add", "new.txt"]);
        let entries = index(&repo);
        let devin = |step| {
            if step == Step::Start {
                write(&repo, "new.txt", b"written by Devin");
            }
        };

        let result = rollback_pausing(repo.path(), dir.path(), &changes(&repo), true, devin);

        assert_interrupted(result, &dir, "new.txt");
        assert_eq!(index(&repo), entries);
    }

    // The previous name did not exist when copying: what appears there is not overwritten either.
    #[test]
    fn what_appears_under_the_previous_name_of_a_rename_is_not_overwritten() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        let contents = b"long content to detect the rename\n";
        repo.commit("old.txt", contents, "one");
        repo.git(&["mv", "old.txt", "new.txt"]);
        let devin = |step| {
            if step == Step::Start {
                write(&repo, "old.txt", b"written by Devin");
            }
        };

        let result = rollback_pausing(repo.path(), dir.path(), &changes(&repo), false, devin);

        assert!(matches!(result, Err(RollbackError::Interrupted { .. })));
        assert_eq!(read(&repo, "old.txt"), b"written by Devin");
        assert!(exists(&repo, "new.txt"));
    }

    #[test]
    fn rollback_modified() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1", "one");
        write(&repo, "a.txt", b"2");

        let id = rollback(repo.path(), dir.path(), &changes(&repo), false).unwrap();

        assert_eq!(read(&repo, "a.txt"), b"1");
        assert!(changes(&repo).is_empty());
        assert_eq!(fs::read(copy_of(&dir, &id, "a.txt")).unwrap(), b"2");
    }

    #[test]
    fn rollback_modified_with_stage_also_undoes_the_stage() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1", "one");
        write(&repo, "a.txt", b"2");
        repo.git(&["add", "a.txt"]);
        write(&repo, "a.txt", b"3");

        let id = rollback(repo.path(), dir.path(), &changes(&repo), false).unwrap();

        assert_eq!(read(&repo, "a.txt"), b"1");
        assert!(changes(&repo).is_empty());
        assert_eq!(fs::read(copy_of(&dir, &id, "a.txt")).unwrap(), b"3");
        let staged = snapshots::stored(dir.path(), &id, "a.txt", true).unwrap();
        assert_eq!(fs::read(staged).unwrap(), b"2");
    }

    #[test]
    fn rollback_keeps_foreign_stage() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1", "one");
        repo.commit("b.txt", b"1", "two");
        write(&repo, "a.txt", b"2");
        write(&repo, "b.txt", b"2");
        repo.git(&["add", "b.txt"]);
        let only_a: Vec<_> = changes(&repo)
            .into_iter()
            .filter(|change| change.path == "a.txt")
            .collect();

        rollback(repo.path(), dir.path(), &only_a, false).unwrap();

        assert_eq!(read(&repo, "a.txt"), b"1");
        assert_eq!(read(&repo, "b.txt"), b"2");
        assert_eq!(repo.git(&["status", "--porcelain"]), "M  b.txt\n");
    }

    #[test]
    fn rollback_new_file_without_deleting() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1", "one");
        write(&repo, "new.txt", b"new");
        repo.git(&["add", "new.txt"]);

        rollback(repo.path(), dir.path(), &changes(&repo), false).unwrap();

        assert_eq!(read(&repo, "new.txt"), b"new");
        assert_eq!(repo.git(&["status", "--porcelain"]), "?? new.txt\n");
    }

    #[test]
    fn rollback_new_file_deleting() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1", "one");
        write(&repo, "new.txt", b"new");
        repo.git(&["add", "new.txt"]);
        write(&repo, "new.txt", "new and more".as_bytes());

        let id = rollback(repo.path(), dir.path(), &changes(&repo), true).unwrap();

        assert!(!exists(&repo, "new.txt"));
        assert!(changes(&repo).is_empty());
        assert_eq!(
            fs::read(copy_of(&dir, &id, "new.txt")).unwrap(),
            "new and more".as_bytes()
        );
    }

    #[test]
    fn rollback_unversioned_file_is_only_deleted_if_requested() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1", "one");
        write(&repo, "loose.txt", b"x");

        rollback(repo.path(), dir.path(), &changes(&repo), false).unwrap();
        assert!(exists(&repo, "loose.txt"));

        let id = rollback(repo.path(), dir.path(), &changes(&repo), true).unwrap();
        assert!(!exists(&repo, "loose.txt"));
        assert_eq!(fs::read(copy_of(&dir, &id, "loose.txt")).unwrap(), b"x");
    }

    #[test]
    fn rollback_deleted() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1", "one");
        repo.commit("b.txt", b"1", "two");
        fs::remove_file(repo.path().join("a.txt")).unwrap();
        repo.git(&["rm", "-q", "b.txt"]);

        rollback(repo.path(), dir.path(), &changes(&repo), false).unwrap();

        assert_eq!(read(&repo, "a.txt"), b"1");
        assert_eq!(read(&repo, "b.txt"), b"1");
        assert!(changes(&repo).is_empty());
    }

    #[test]
    fn rollback_renamed() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        let contents = b"long content to detect the rename\n";
        repo.commit("old.txt", contents, "one");
        repo.git(&["mv", "old.txt", "new.txt"]);
        let all = changes(&repo);
        assert_eq!(all[0].state, FileState::Renamed);

        let id = rollback(repo.path(), dir.path(), &all, false).unwrap();

        assert_eq!(read(&repo, "old.txt"), contents);
        assert!(!exists(&repo, "new.txt"));
        assert!(changes(&repo).is_empty());
        let snapshot = snapshots::list(dir.path())
            .into_iter()
            .find(|s| s.id == id)
            .unwrap();
        let by_path = |path: &str| snapshot.entries.iter().find(|e| e.path == path).unwrap();
        assert_eq!(by_path("new.txt").kind, EntryKind::File);
        assert_eq!(by_path("old.txt").kind, EntryKind::Absent);
        assert_eq!(fs::read(copy_of(&dir, &id, "new.txt")).unwrap(), contents);
    }

    #[test]
    fn rollback_of_copy_does_not_touch_the_source() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("source.txt", b"long content to detect the copy\n", "one");
        fs::copy(repo.path().join("source.txt"), repo.path().join("copy.txt")).unwrap();
        repo.git(&["add", "copy.txt"]);
        write(&repo, "source.txt", b"changed");
        let only_copy: Vec<_> = changes(&repo)
            .into_iter()
            .filter(|change| change.path == "copy.txt")
            .collect();

        rollback(repo.path(), dir.path(), &only_copy, false).unwrap();

        assert_eq!(read(&repo, "source.txt"), b"changed");
    }

    #[test]
    fn restore_snapshot() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1", "one");
        write(&repo, "a.txt", "unsaved work".as_bytes());
        let id = rollback(repo.path(), dir.path(), &changes(&repo), false).unwrap();
        assert_eq!(read(&repo, "a.txt"), b"1");

        snapshots::restore(dir.path(), repo.path(), &id).unwrap();

        assert_eq!(read(&repo, "a.txt"), "unsaved work".as_bytes());
    }

    #[test]
    fn if_the_snapshot_fails_it_touches_nothing() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1", "one");
        repo.commit("b.txt", b"1", "two");
        write(&repo, "a.txt", b"2");
        write(&repo, "new.txt", b"new");
        repo.git(&["add", "new.txt"]);
        let before = repo.git(&["status", "--porcelain"]);
        let root = dir.path().join("snapshots");
        fs::create_dir(&root).unwrap();
        let original = fs::metadata(&root).unwrap().permissions();
        let mut locked = original.clone();
        std::os::unix::fs::PermissionsExt::set_mode(&mut locked, 0o500);
        fs::set_permissions(&root, locked).unwrap();

        let result = rollback(repo.path(), dir.path(), &changes(&repo), true);

        fs::set_permissions(&root, original).unwrap();
        assert!(
            matches!(result, Err(RollbackError::Snapshot(_))),
            "{result:?}"
        );
        assert_eq!(read(&repo, "a.txt"), b"2");
        assert_eq!(read(&repo, "new.txt"), b"new");
        assert_eq!(repo.git(&["status", "--porcelain"]), before);
    }

    #[test]
    fn rollback_without_commits() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        write(&repo, "a.txt", b"new");
        write(&repo, "loose.txt", b"x");
        repo.git(&["add", "a.txt"]);
        let only_a: Vec<_> = changes(&repo)
            .into_iter()
            .filter(|change| change.path == "a.txt")
            .collect();

        let id = rollback(repo.path(), dir.path(), &only_a, false).unwrap();

        assert_eq!(read(&repo, "a.txt"), b"new");
        assert_eq!(
            repo.git(&["status", "--porcelain"]),
            "?? a.txt\n?? loose.txt\n"
        );
        assert_eq!(fs::read(copy_of(&dir, &id, "a.txt")).unwrap(), b"new");
    }

    #[test]
    fn rollback_without_commits_deleting() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        write(&repo, "a.txt", b"new");
        repo.git(&["add", "a.txt"]);
        write(&repo, "a.txt", "new and more".as_bytes());

        let id = rollback(repo.path(), dir.path(), &changes(&repo), true).unwrap();

        assert!(!exists(&repo, "a.txt"));
        assert_eq!(repo.git(&["status", "--porcelain"]), "");
        assert_eq!(
            fs::read(copy_of(&dir, &id, "a.txt")).unwrap(),
            "new and more".as_bytes()
        );
    }

    #[test]
    fn rolling_back_a_path_behind_a_link_is_rejected_and_does_not_touch_the_link() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("d/f", b"1\n", "base");
        fs::remove_dir_all(repo.path().join("d")).unwrap();
        fs::create_dir(repo.path().join("e")).unwrap();
        symlink("e", repo.path().join("d")).unwrap();

        let result = rollback_paths(repo.path(), dir.path(), &paths(&["d/f"]), true);

        let error = result.unwrap_err();
        assert!(matches!(error, RollbackError::Displaced(_)), "{error:?}");
        assert!(error.to_string().contains("d/f"), "{error}");
        let meta = fs::symlink_metadata(repo.path().join("d")).unwrap();
        assert!(meta.file_type().is_symlink());
        assert!(snapshots::list(dir.path()).is_empty());
    }

    #[test]
    fn path_outside_the_repository_is_rejected() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("x.txt"), "outside").unwrap();
        symlink(outside.path(), repo.path().join("outlink")).unwrap();
        repo.commit("a.txt", b"1", "one");
        write(&repo, "a.txt", b"2");
        let change = |path: &str| FileChange {
            path: path.to_string(),
            old_path: None,
            state: FileState::Untracked,
            staged: false,
            unstaged: true,
        };
        let mut all = changes(&repo);
        all.push(change("outlink/x.txt"));

        for bad in [
            "../outside.txt",
            "/etc/passwd",
            "outlink/x.txt",
            ".git/config",
        ] {
            let mut with_bad = changes(&repo);
            with_bad.push(change(bad));
            let result = rollback(repo.path(), dir.path(), &with_bad, true);
            assert!(
                matches!(result, Err(RollbackError::OutsideRepository(_))),
                "{bad}"
            );
        }

        let mut renamed = changes(&repo);
        renamed.push(FileChange {
            old_path: Some("../outside.txt".to_string()),
            state: FileState::Renamed,
            ..change("b.txt")
        });
        let result = rollback(repo.path(), dir.path(), &renamed, true);
        assert!(matches!(result, Err(RollbackError::OutsideRepository(_))));

        assert_eq!(read(&repo, "a.txt"), b"2");
        assert_eq!(fs::read(outside.path().join("x.txt")).unwrap(), b"outside");
        assert!(snapshots::list(dir.path()).is_empty());
    }

    #[test]
    fn rollback_treats_paths_as_literals() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        // As a pattern, `:(exclude)ab.txt` would mean "everything but ab.txt".
        let magic = ":(exclude)ab.txt";
        repo.commit(magic, b"1", "one");
        repo.commit("ab.txt", b"1", "two");
        repo.commit("c.txt", b"1", "three");
        for file in [magic, "ab.txt", "c.txt"] {
            write(&repo, file, b"2");
        }
        let only_magic: Vec<_> = changes(&repo)
            .into_iter()
            .filter(|change| change.path == magic)
            .collect();

        rollback(repo.path(), dir.path(), &only_magic, false).unwrap();

        assert_eq!(read(&repo, magic), b"1");
        assert_eq!(read(&repo, "ab.txt"), b"2");
        assert_eq!(read(&repo, "c.txt"), b"2");
    }

    #[test]
    fn rollback_without_changes_does_nothing() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit(".gitignore", b"*.log\n", "one");
        write(&repo, "debug.log", b"x");
        let ignored = status::read(repo.path(), true).unwrap();

        let result = rollback(repo.path(), dir.path(), &ignored, true);

        assert!(matches!(result, Err(RollbackError::NothingToRollback)));
        assert!(exists(&repo, "debug.log"));
        assert!(snapshots::list(dir.path()).is_empty());
    }

    #[test]
    fn rollback_by_paths_only_touches_the_requested_ones() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1", "one");
        repo.commit("b.txt", b"1", "two");
        write(&repo, "a.txt", b"2");
        write(&repo, "b.txt", b"2");

        rollback_paths(
            repo.path(),
            dir.path(),
            &paths(&["a.txt", "does-not-exist.txt"]),
            false,
        )
        .unwrap();

        assert_eq!(read(&repo, "a.txt"), b"1");
        assert_eq!(read(&repo, "b.txt"), b"2");
        let none = rollback_paths(
            repo.path(),
            dir.path(),
            &paths(&["does-not-exist.txt"]),
            false,
        );
        assert!(matches!(none, Err(RollbackError::NothingToRollback)));
    }

    // A real merge that leaves a conflict of the requested kind on `f.txt`.
    fn conflict(kind: &str) -> TempRepo {
        let repo = TempRepo::new();
        match kind {
            "AA" => {
                repo.commit("base.txt", b"base\n", "base");
                repo.git(&["checkout", "-q", "-b", "other"]);
                repo.commit("f.txt", b"other\n", "other");
                repo.git(&["checkout", "-q", "main"]);
                repo.commit("f.txt", b"main\n", "main");
            }
            _ => {
                repo.commit("f.txt", b"base\n", "base");
                repo.git(&["checkout", "-q", "-b", "other"]);
                if kind == "UD" {
                    repo.git(&["rm", "-q", "f.txt"]);
                    repo.git(&["commit", "-q", "-m", "other"]);
                } else {
                    repo.commit("f.txt", b"other\n", "other");
                }
                repo.git(&["checkout", "-q", "main"]);
                if kind == "DU" {
                    repo.git(&["rm", "-q", "f.txt"]);
                    repo.git(&["commit", "-q", "-m", "main"]);
                } else {
                    repo.commit("f.txt", b"main\n", "main");
                }
            }
        }
        let merge = std::process::Command::new("git")
            .current_dir(repo.path())
            .args(["merge", "--no-edit", "other"])
            .output()
            .unwrap();
        assert!(!merge.status.success());
        let status = repo.git(&["status", "--porcelain"]);
        assert!(status.starts_with(&format!("{kind} f.txt")), "{status}");
        repo
    }

    fn assert_conflict_untouched(kind: &str) {
        let repo = conflict(kind);
        let dir = TempDir::new().unwrap();
        let status_before = repo.git(&["status", "--porcelain"]);
        let disk_before = fs::read(repo.path().join("f.txt")).ok();

        for delete_added in [false, true] {
            let result = rollback(repo.path(), dir.path(), &changes(&repo), delete_added);

            let listed = matches!(&result, Err(RollbackError::Unmerged(p)) if p == &["f.txt"]);
            assert!(listed, "{kind}: {result:?}");
            assert_eq!(
                repo.git(&["status", "--porcelain"]),
                status_before,
                "{kind}"
            );
            assert_eq!(
                fs::read(repo.path().join("f.txt")).ok(),
                disk_before,
                "{kind}"
            );
            repo.git(&["rev-parse", "-q", "--verify", "MERGE_HEAD"]);
            assert!(snapshots::list(dir.path()).is_empty(), "{kind}");
        }
    }

    #[test]
    fn rollback_rejects_conflict_uu() {
        assert_conflict_untouched("UU");
    }

    #[test]
    fn rollback_rejects_conflict_aa() {
        assert_conflict_untouched("AA");
    }

    #[test]
    fn rollback_rejects_conflict_du() {
        assert_conflict_untouched("DU");
    }

    #[test]
    fn rollback_rejects_conflict_ud() {
        assert_conflict_untouched("UD");
    }

    #[test]
    fn rollback_of_conflict_touches_nothing() {
        let repo = conflict("UU");
        let dir = TempDir::new().unwrap();
        write(&repo, "loose.txt", b"x");
        let status_before = repo.git(&["status", "--porcelain"]);

        let result = rollback(repo.path(), dir.path(), &changes(&repo), true);

        assert!(
            matches!(result, Err(RollbackError::Unmerged(_))),
            "{result:?}"
        );
        assert_eq!(read(&repo, "loose.txt"), b"x");
        assert_eq!(repo.git(&["status", "--porcelain"]), status_before);
        assert!(read(&repo, "f.txt").starts_with(b"<<<<<<<"));
        repo.git(&["rev-parse", "-q", "--verify", "MERGE_HEAD"]);
        assert!(snapshots::list(dir.path()).is_empty());
    }

    #[test]
    fn rollback_rename_case_only() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        let contents = b"long content to detect the rename\n";
        repo.commit("a.txt", contents, "one");
        repo.git(&["mv", "a.txt", "A.txt"]);
        write(&repo, "A.txt", b"long content to detect the rename\nmore\n");
        let all = changes(&repo);
        assert!(all.iter().any(|c| c.state == FileState::Renamed), "{all:?}");

        rollback(repo.path(), dir.path(), &all, false).unwrap();

        assert_eq!(read(&repo, "a.txt"), contents);
        assert_eq!(repo.git(&["status", "--porcelain"]), "");
        assert_eq!(repo.git(&["ls-files"]), "a.txt\n");
    }

    #[test]
    fn rollback_of_name_with_another_unicode_normalization_does_not_delete() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        let nfd = "e\u{301}.txt";
        write(&repo, nfd, b"1");
        repo.git(&["-c", "core.precomposeunicode=false", "add", "-A"]);
        repo.git(&["commit", "-q", "-m", "one"]);
        repo.git(&["config", "core.precomposeunicode", "true"]);
        write(&repo, nfd, b"2");
        let modified: Vec<_> = changes(&repo)
            .into_iter()
            .filter(|change| change.state == FileState::Modified)
            .collect();
        assert_eq!(modified.len(), 1, "{modified:?}");

        let result = rollback(repo.path(), dir.path(), &modified, true);

        assert!(exists(&repo, nfd));
        match result {
            Err(error) => assert!(matches!(error, RollbackError::NotInHead(_)), "{error:?}"),
            Ok(_) => assert_eq!(read(&repo, nfd), b"1"),
        }
    }

    fn fake_change(path: &str, old_path: Option<&str>, state: FileState) -> FileChange {
        FileChange {
            path: path.to_string(),
            old_path: old_path.map(str::to_string),
            state,
            staged: false,
            unstaged: true,
        }
    }

    #[test]
    fn rollback_of_modified_or_deleted_that_head_does_not_have_is_aborted() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1", "one");
        write(&repo, "ghost.txt", b"x");

        for state in [FileState::Modified, FileState::Deleted] {
            let change = fake_change("ghost.txt", None, state);
            let result = rollback(repo.path(), dir.path(), &[change], true);

            let listed = matches!(&result, Err(RollbackError::NotInHead(p)) if p == &["ghost.txt"]);
            assert!(listed, "{state:?}: {result:?}");
            assert_eq!(read(&repo, "ghost.txt"), b"x");
        }
        assert!(snapshots::list(dir.path()).is_empty());
    }

    #[test]
    fn rollback_of_rename_with_source_outside_head_does_not_delete_the_new_one() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1", "one");
        write(&repo, "new.txt", b"x");
        let change = fake_change("new.txt", Some("ghost.txt"), FileState::Renamed);

        for delete_added in [false, true] {
            let result = rollback(
                repo.path(),
                dir.path(),
                std::slice::from_ref(&change),
                delete_added,
            );

            assert!(
                matches!(result, Err(RollbackError::NotInHead(_))),
                "{result:?}"
            );
            assert_eq!(read(&repo, "new.txt"), b"x");
        }
        assert!(snapshots::list(dir.path()).is_empty());
    }

    #[test]
    fn rollback_only_deletes_new_or_unversioned_files() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1", "one");
        write(&repo, "copy.txt", b"x");
        repo.git(&["add", "copy.txt"]);
        let copied = fake_change("copy.txt", Some("a.txt"), FileState::Copied);

        rollback(repo.path(), dir.path(), &[copied], true).unwrap();

        assert_eq!(read(&repo, "copy.txt"), b"x");
        assert_eq!(repo.git(&["status", "--porcelain"]), "?? copy.txt\n");
    }

    fn case_sensitive_base() -> Option<std::path::PathBuf> {
        let base = std::path::PathBuf::from(std::env::var_os("RAMA_CASE_SENSITIVE_DIR")?);
        let probe = tempfile::TempDir::new_in(&base).unwrap();
        fs::write(probe.path().join("a"), "").unwrap();
        (!probe.path().join("A").exists()).then_some(base)
    }

    // It only runs if RAMA_CASE_SENSITIVE_DIR points to a folder on a volume that distinguishes
    // case; no volume is created.
    #[test]
    fn rollback_of_paths_differing_only_in_case_on_a_case_sensitive_volume() {
        let Some(base) = case_sensitive_base() else {
            eprintln!("skipped: RAMA_CASE_SENSITIVE_DIR does not point to a case-sensitive volume");
            return;
        };
        let repo = TempRepo::new_in(&base);
        let dir = TempDir::new().unwrap();
        repo.commit("base.txt", b"1", "one");
        write(&repo, "Notes.txt", b"one");
        write(&repo, "notes.txt", b"two");

        let id = rollback(repo.path(), dir.path(), &changes(&repo), true).unwrap();

        assert!(!exists(&repo, "Notes.txt") && !exists(&repo, "notes.txt"));
        snapshots::restore(dir.path(), repo.path(), &id).unwrap();
        assert_eq!(read(&repo, "Notes.txt"), b"one");
        assert_eq!(read(&repo, "notes.txt"), b"two");
    }

    #[test]
    fn rollback_rename_case_only_rewritten() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        let contents = b"long content to detect the rename\n";
        repo.commit("a.txt", contents, "one");
        repo.git(&["mv", "a.txt", "A.txt"]);
        write(&repo, "A.txt", b"nothing to do with the previous one\n");
        repo.git(&["add", "A.txt"]);
        let all = changes(&repo);
        assert!(
            all.iter().all(|c| c.state != FileState::Renamed),
            "git should not have detected a rename: {all:?}"
        );

        rollback(repo.path(), dir.path(), &all, true).unwrap();

        assert_eq!(read(&repo, "a.txt"), contents);
        assert_eq!(repo.git(&["status", "--porcelain"]), "");
        assert_eq!(repo.git(&["ls-files"]), "a.txt\n");
    }

    // Decomposed name (NFD) built with explicit bytes, and its composed form (NFC).
    fn nfd_name() -> String {
        String::from_utf8(b"e\xcc\x81.txt".to_vec()).unwrap()
    }

    fn nfc_name() -> String {
        String::from_utf8(b"\xc3\xa9.txt".to_vec()).unwrap()
    }

    // Repository with a versioned file with an NFD name and `precomposeunicode=true`, as in
    // a default macOS clone: git also shows a ghost entry `?? é.txt` (NFC).
    fn repo_with_nfd_name() -> TempRepo {
        let repo = TempRepo::new();
        write(&repo, &nfd_name(), b"1");
        repo.git(&["-c", "core.precomposeunicode=false", "add", "-A"]);
        repo.git(&["commit", "-q", "-m", "one"]);
        repo.git(&["config", "core.precomposeunicode", "true"]);
        repo
    }

    #[test]
    fn rollback_of_ghost_nfd_entry_does_not_delete() {
        let repo = repo_with_nfd_name();
        let dir = TempDir::new().unwrap();
        let all = changes(&repo);
        let phantom: Vec<_> = all
            .iter()
            .filter(|change| change.state == FileState::Untracked)
            .cloned()
            .collect();
        assert_eq!(phantom.len(), 1, "{all:?}");
        assert_eq!(phantom[0].path, nfc_name());

        let result = rollback(repo.path(), dir.path(), &phantom, true);

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(read(&repo, &nfd_name()), b"1");
        assert_eq!(
            repo.git(&["ls-files", "-z"]).trim_end_matches('\0'),
            nfd_name()
        );
        assert!(!repo.git(&["status", "--porcelain"]).contains(" D "));
    }

    #[test]
    fn rollback_of_full_selection_with_ghost_entry_does_not_delete() {
        let repo = repo_with_nfd_name();
        let dir = TempDir::new().unwrap();
        write(&repo, &nfd_name(), b"2");
        let all = changes(&repo);
        assert!(
            all.iter().any(|c| c.state == FileState::Untracked),
            "{all:?}"
        );

        let result = rollback(repo.path(), dir.path(), &all, true);

        assert!(exists(&repo, &nfd_name()));
        match result {
            Err(error) => assert!(matches!(error, RollbackError::NotInHead(_)), "{error:?}"),
            Ok(_) => assert_eq!(read(&repo, &nfd_name()), b"1"),
        }
    }

    // The two names are the same file on a volume that fully folds case
    // (APFS by default: `ß` and `SS`); on a case-sensitive one they are different files.
    fn folds_sharp_s() -> bool {
        let probe = TempDir::new().unwrap();
        fs::write(probe.path().join("Stra\u{df}e.txt"), "x").unwrap();
        fs::symlink_metadata(probe.path().join("STRASSE.txt")).is_ok()
    }

    #[test]
    fn rollback_name_with_full_folding_does_not_delete() {
        if !folds_sharp_s() {
            eprintln!("skipped: the volume is case-sensitive");
            return;
        }
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        let tracked = "Stra\u{df}e.txt";
        repo.commit(tracked, b"1", "one");
        fs::rename(repo.path().join(tracked), repo.path().join("STRASSE.txt")).unwrap();
        let all = changes(&repo);
        assert!(all.iter().any(|c| c.path == "STRASSE.txt"), "{all:?}");

        let result = rollback(repo.path(), dir.path(), &all, true);

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(read(&repo, "STRASSE.txt"), b"1");
        assert_eq!(read(&repo, tracked), b"1");
        assert_eq!(
            repo.git(&["ls-files", "-z"]).trim_end_matches('\0'),
            tracked
        );
    }

    // Without reading HEAD in the guard, `a.txt` (only in HEAD) would not be seen and `A.txt` would be deleted.
    #[test]
    fn rollback_of_only_the_new_name_of_a_case_rename_does_not_delete() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"long content to detect the rename\n", "one");
        repo.git(&["mv", "a.txt", "A.txt"]);
        write(&repo, "A.txt", b"nothing to do with the previous one\n");
        repo.git(&["add", "A.txt"]);
        let only_new: Vec<_> = changes(&repo)
            .into_iter()
            .filter(|change| change.path == "A.txt")
            .collect();
        assert_eq!(only_new.len(), 1);

        let result = rollback(repo.path(), dir.path(), &only_new, true);

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(
            read(&repo, "A.txt"),
            b"nothing to do with the previous one\n"
        );
        assert_eq!(
            read(&repo, "a.txt"),
            b"nothing to do with the previous one\n"
        );
    }

    fn only(repo: &TempRepo, names: &[&str]) -> Vec<FileChange> {
        changes(repo)
            .into_iter()
            .filter(|change| names.contains(&change.path.as_str()))
            .collect()
    }

    // `foo/bar.txt` is still versioned, but `foo` is now a file: its `lstat` gives ENOTDIR.
    fn replace_folder_with_file(repo: &TempRepo) {
        fs::remove_dir_all(repo.path().join("foo")).unwrap();
        write(repo, "foo", b"I am a file");
    }

    #[test]
    fn rollback_of_new_file_with_a_versioned_folder_replaced_by_a_file() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("foo/bar.txt", b"1", "one");
        replace_folder_with_file(&repo);
        write(&repo, "new.txt", b"new");
        repo.git(&["add", "new.txt"]);
        let selected = only(&repo, &["new.txt"]);

        let result = rollback(repo.path(), dir.path(), &selected, true);

        assert!(result.is_ok(), "{result:?}");
        assert!(!exists(&repo, "new.txt"));
        assert_eq!(read(&repo, "foo"), b"I am a file");
    }

    #[test]
    fn rollback_of_rename_with_a_versioned_folder_replaced_by_a_file() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("foo/bar.txt", b"1", "one");
        repo.commit("r.txt", b"long content to detect the rename\n", "two");
        repo.git(&["mv", "r.txt", "s.txt"]);
        replace_folder_with_file(&repo);
        let selected = only(&repo, &["s.txt"]);
        assert_eq!(selected[0].state, FileState::Renamed);

        let result = rollback(repo.path(), dir.path(), &selected, false);

        assert!(result.is_ok(), "{result:?}");
        assert!(exists(&repo, "r.txt"));
        assert!(!exists(&repo, "s.txt"));
        assert_eq!(read(&repo, "foo"), b"I am a file");
    }

    #[test]
    fn if_the_versioned_files_cannot_be_read_the_rename_is_not_left_half_done() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("closed/x.txt", b"1", "one");
        repo.commit("r.txt", b"long content to detect the rename\n", "two");
        repo.git(&["mv", "r.txt", "s.txt"]);
        let selected = only(&repo, &["s.txt"]);
        let before = repo.git(&["status", "--porcelain"]);
        let folder = repo.path().join("closed");
        let original = fs::metadata(&folder).unwrap().permissions();
        let mut locked = original.clone();
        std::os::unix::fs::PermissionsExt::set_mode(&mut locked, 0o000);
        fs::set_permissions(&folder, locked).unwrap();

        let result = rollback(repo.path(), dir.path(), &selected, false);

        fs::set_permissions(&folder, original).unwrap();
        assert!(
            matches!(result, Err(RollbackError::Failed(_))),
            "{result:?}"
        );
        assert!(!exists(&repo, "r.txt"));
        assert!(exists(&repo, "s.txt"));
        assert_eq!(repo.git(&["status", "--porcelain"]), before);
        assert!(snapshots::list(dir.path()).is_empty());
    }

    #[test]
    fn a_failure_in_the_check_does_not_leave_the_deletion_half_done() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        write(&repo, "first.txt", b"x");
        write(&repo, "second.txt", b"x");
        let plan = Plan {
            delete: vec![
                Deletion {
                    path: "first.txt".to_string(),
                    restored_from: None,
                },
                Deletion {
                    path: "second.txt".to_string(),
                    restored_from: Some("not-restored.txt".to_string()),
                },
            ],
            ..Plan::default()
        };

        let result = apply(repo.path(), &plan, &|_, _| Ok(()));

        assert!(result.is_err(), "{result:?}");
        assert!(exists(&repo, "first.txt"));
        assert!(exists(&repo, "second.txt"));
    }

    #[test]
    fn deleting_a_name_under_a_file_is_not_an_error() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        write(&repo, "foo", b"I am a file");
        let plan = Plan {
            delete: vec![Deletion {
                path: "foo/bar.txt".to_string(),
                restored_from: None,
            }],
            ..Plan::default()
        };

        let result = apply(repo.path(), &plan, &|_, _| Ok(()));

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(read(&repo, "foo"), b"I am a file");
    }

    // `a.txt` to rewritten `A.txt` (the same file as HEAD's `a.txt`) and the damaged branch.
    fn repo_with_damaged_branch(contents: &[u8]) -> (TempRepo, Vec<FileChange>) {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"long content to detect the rename\n", "one");
        repo.git(&["mv", "a.txt", "A.txt"]);
        write(&repo, "A.txt", b"nothing to do with the previous one\n");
        repo.git(&["add", "A.txt"]);
        let selected = only(&repo, &["A.txt"]);
        assert_eq!(selected.len(), 1);
        fs::write(repo.path().join(".git/refs/heads/main"), contents).unwrap();
        (repo, selected)
    }

    #[test]
    fn rollback_with_the_damaged_branch_touches_nothing() {
        for contents in DAMAGED_REFS {
            let (repo, selected) = repo_with_damaged_branch(contents);
            let dir = TempDir::new().unwrap();
            let index = repo.git(&["ls-files"]);

            let result = rollback(repo.path(), dir.path(), &selected, true);

            let failed = matches!(result, Err(RollbackError::Failed(_)));
            assert!(failed, "{contents:?}: {result:?}");
            assert_eq!(
                read(&repo, "A.txt"),
                b"nothing to do with the previous one\n"
            );
            assert_eq!(repo.git(&["ls-files"]), index);
            assert!(snapshots::list(dir.path()).is_empty());
        }
    }

    #[test]
    fn rollback_by_paths_with_the_damaged_branch_touches_nothing() {
        for contents in DAMAGED_REFS {
            let (repo, _) = repo_with_damaged_branch(contents);
            let dir = TempDir::new().unwrap();
            let index = repo.git(&["ls-files"]);

            let result = rollback_paths(repo.path(), dir.path(), &paths(&["A.txt"]), true);

            let failed = matches!(result, Err(RollbackError::Failed(_)));
            assert!(failed, "{contents:?}: {result:?}");
            assert_eq!(
                read(&repo, "A.txt"),
                b"nothing to do with the previous one\n"
            );
            assert_eq!(repo.git(&["ls-files"]), index);
            assert!(snapshots::list(dir.path()).is_empty());
        }
    }

    #[test]
    fn rollback_does_not_continue_if_it_cannot_copy_a_folder() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1\n", "base");
        fs::remove_file(repo.path().join("a.txt")).unwrap();
        write(&repo, "a.txt/inside", b"unversioned\n");

        let result = rollback_paths(repo.path(), dir.path(), &paths(&["a.txt"]), false);

        assert!(matches!(result, Err(RollbackError::Snapshot(_))));
        assert_eq!(read(&repo, "a.txt/inside"), b"unversioned\n");
    }
}
