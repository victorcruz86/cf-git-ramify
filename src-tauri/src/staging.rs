use std::collections::BTreeMap;
use std::path::Path;

use crate::branches::Repo;
use crate::commit::{self, CommitRequest, PartialFile, Saved};
use crate::diff;
use crate::git::{self, GitError};
use crate::hunks::{self, HunkError, LineSelection, Sides};
use crate::snapshots::{self, SnapshotId};

pub const BEFORE_STAGE: &str = "before changing the stage";
pub const BEFORE_REVERT: &str = "before reverting to the stage";

// What is binary or is not a regular file is not split into lines.
fn splittable(bytes: Option<Vec<u8>>) -> Option<Vec<u8>> {
    bytes.filter(|bytes| !diff::looks_binary(bytes))
}

// HEAD and the index.
type StagedPair = (Vec<u8>, Vec<u8>);

// What would be committed. `None` if the file is treated as a whole.
pub fn staged_view(repo: &Path, path: &str) -> Result<Option<StagedPair>, HunkError> {
    hunks::check_path(repo, path)?;
    let Some(staged) = splittable(hunks::read_index(repo, path)?) else {
        return Ok(None);
    };
    let head = splittable(hunks::read_head(repo, path)?);
    Ok(head.map(|head| (head, staged)))
}

// What remains to be staged: the index and the file in the form git stores. `None` if the
// file is treated as a whole, also if it is not on disk.
pub fn unstaged_view(repo: &Path, path: &str) -> Result<Option<(Vec<u8>, Sides)>, HunkError> {
    hunks::check_path(repo, path)?;
    let Some(staged) = splittable(hunks::read_index(repo, path)?) else {
        return Ok(None);
    };
    Ok(hunks::read_sides(repo, path)?.map(|sides| (staged, sides)))
}

fn snapshot(repo: &Path, dir: &Path, paths: &[String]) -> Result<SnapshotId, HunkError> {
    snapshots::create_for_index(dir, repo, paths, BEFORE_STAGE).map_err(HunkError::Snapshot)
}

fn check_paths(repo: &Path, paths: &[String]) -> Result<(), HunkError> {
    for path in paths {
        hunks::check_path(repo, path)?;
    }
    match commit::conflicted(repo, paths)?.into_iter().next() {
        Some(path) => Err(HunkError::Unmerged(path)),
        None => Ok(()),
    }
}

// A snapshot is of files: one cannot be made of a submodule, and it must not prevent the operation.
fn copyable(repo: &Path, path: &str) -> Result<bool, HunkError> {
    Ok(hunks::index_gitlink(repo, path)?.is_none())
}

// The paths that would lose lines that are only in the index. What cannot be examined by lines
// counts as lost.
fn at_risk(repo: &Path, paths: &[String]) -> Result<Vec<String>, HunkError> {
    let mut risky = Vec::new();
    // `git add` removes from the index what clashes with the path it adds.
    let mut candidates = snapshots::index_clashes(repo, paths)?;
    candidates.extend(paths.iter().cloned());
    let candidates: Vec<String> = candidates.into_iter().collect();
    for path in snapshots::staged_only(repo, &candidates)? {
        if !copyable(repo, &path)? {
            continue;
        }
        // Behind a link `git add` fails, and what is on the disk does not belong to this path.
        let unstaged = if snapshots::behind_other(repo, &path) {
            None
        } else {
            unstaged_view(repo, &path)?
        };
        let lost = match (staged_view(repo, &path)?, unstaged) {
            (Some((head, staged)), Some((_, sides))) => {
                hunks::loses_lines(&staged, &[&head, &sides.stored])
            }
            _ => true,
        };
        if lost {
            risky.push(path);
        }
    }
    Ok(risky)
}

fn copy_at_risk(
    repo: &Path,
    dir: &Path,
    paths: &[String],
) -> Result<Option<SnapshotId>, HunkError> {
    let risky = at_risk(repo, paths)?;
    if risky.is_empty() {
        return Ok(None);
    }
    snapshot(repo, dir, &risky).map(Some)
}

// Without its own lock: the caller holds the operations one and has gone through `commit::ensure_idle`.
// All of them return the snapshot they saved, if there was something that was only in the index.
pub fn stage(repo: &Path, dir: &Path, paths: &[String]) -> Result<Option<SnapshotId>, HunkError> {
    check_paths(repo, paths)?;
    let copy = copy_at_risk(repo, dir, paths)?;
    // Behind a link, `git add` fails entirely; for the index, that file is no longer there.
    let (removed, added) = paths
        .iter()
        .cloned()
        .partition(|path| snapshots::behind_other(repo, path));
    let request = CommitRequest {
        added,
        removed,
        ..Default::default()
    };
    commit::stage_selection(repo, &request, &commit::read_entries(repo, paths)?)?;
    Ok(copy)
}

fn git_error(error: HunkError) -> GitError {
    match error {
        HunkError::Git(error) => error,
        other => GitError::Io(other.to_string()),
    }
}

// `ChangesView.AddUnversioned` with changelists: `git add --ignore-errors -A`, as
// `GitFileUtils.addPathsImpl`. It runs in a write task, so it holds the operations lock; like every
// other change to the index, it first gives back the stage of an interrupted commit. What is new is
// looked up with the name `add` reads from disk, as in `commit::stage_selection`.
pub fn add_to_vcs(repo: &Repo, paths: &[String]) -> Result<Option<SnapshotId>, GitError> {
    commit::ensure_idle(repo.path, repo.data).map_err(|error| match error {
        commit::CommitError::Git(error) => error,
        other => GitError::Io(other.to_string()),
    })?;
    check_paths(repo.path, paths).map_err(git_error)?;
    let copy = copy_at_risk(repo.path, repo.data, paths).map_err(git_error)?;
    for chunk in paths.chunks(git::PATHS_PER_COMMAND) {
        let mut args = vec!["--literal-pathspecs", "add", "--ignore-errors", "-A", "--"];
        args.extend(chunk.iter().map(String::as_str));
        repo.ctx.git(&args)?;
    }
    Ok(copy)
}

pub fn unstage(repo: &Path, dir: &Path, paths: &[String]) -> Result<Option<SnapshotId>, HunkError> {
    check_paths(repo, paths)?;
    let copy = copy_at_risk(repo, dir, paths)?;
    commit::unstage(repo, git::head_exists(repo)?, paths)?;
    Ok(copy)
}

// `Git.Stage.Revert`: the disk goes back to what is in the index, with `git checkout -- paths`. Before that it
// always saves a snapshot of the files, with what was not anywhere else. If in the
// place of a file there is something else it does not continue: `checkout` would delete it whole, with whatever it has
// unversioned, and `git status` does not even show it.
pub fn revert(repo: &Path, dir: &Path, paths: &[String]) -> Result<Option<SnapshotId>, HunkError> {
    check_paths(repo, paths)?;
    let mut files = Vec::new();
    for path in paths {
        if !copyable(repo, path)? {
            continue;
        }
        if snapshots::displaced(repo, path) {
            return Err(HunkError::Displaced(path.clone()));
        }
        files.push(path.clone());
    }
    let copy = if files.is_empty() {
        None
    } else {
        let snapshot = snapshots::create(dir, repo, &files, BEFORE_REVERT);
        Some(snapshot.map_err(HunkError::Snapshot)?)
    };
    git::git_with_paths(repo, true, &git::exact(&["checkout"]), paths)?;
    Ok(copy)
}

// An entry without a snapshot of its lost lines is never written.
fn copy_if_lost(
    repo: &Path,
    dir: &Path,
    path: &str,
    staged: &[u8],
    kept: &[&[u8]],
) -> Result<Option<SnapshotId>, HunkError> {
    if !hunks::loses_lines(staged, kept) {
        return Ok(None);
    }
    snapshot(repo, dir, &[path.to_string()]).map(Some)
}

fn set_index(
    repo: &Path,
    path: &str,
    content: Vec<u8>,
    saved: &BTreeMap<String, Saved>,
) -> Result<(), HunkError> {
    let file = PartialFile {
        path: path.to_string(),
        content,
    };
    commit::write_partial(repo, &[file], saved)?;
    Ok(())
}

// Takes to the index what was chosen from what remains to be staged: the base is the index and the target, the
// file in the form git stores.
pub fn stage_hunks(
    repo: &Path,
    dir: &Path,
    path: &str,
    selection: &[LineSelection],
) -> Result<Option<SnapshotId>, HunkError> {
    let (staged, sides) =
        unstaged_view(repo, path)?.ok_or_else(|| HunkError::NotSplittable(path.to_string()))?;
    hunks::check_selection(path, selection, &hunks::compute(&staged, &sides.stored))?;
    if hunks::selects_nothing(selection) {
        return Err(HunkError::NothingSelected);
    }
    let result = hunks::partial_content(&staged, &sides.stored, selection);
    let copy = copy_if_lost(
        repo,
        dir,
        path,
        &staged,
        &[&result, &sides.base, &sides.stored],
    )?;
    let paths = [path.to_string()];
    set_index(repo, path, result, &commit::read_entries(repo, &paths)?)?;
    Ok(copy)
}

// Takes out of the index what was chosen from what is staged: the base is HEAD and the target, the index.
pub fn unstage_hunks(
    repo: &Path,
    dir: &Path,
    path: &str,
    selection: &[LineSelection],
) -> Result<Option<SnapshotId>, HunkError> {
    let (head, staged) =
        staged_view(repo, path)?.ok_or_else(|| HunkError::NotSplittable(path.to_string()))?;
    hunks::check_selection(path, selection, &hunks::compute(&head, &staged))?;
    if hunks::selects_nothing(selection) {
        return Err(HunkError::NothingSelected);
    }
    let result = hunks::revert_content(&head, &staged, selection);
    let stored = hunks::read_sides(repo, path)?.map(|sides| sides.stored);
    let on_disk = stored.as_deref().unwrap_or_default();
    let copy = copy_if_lost(repo, dir, path, &staged, &[&result, &head, on_disk])?;
    let paths = [path.to_string()];
    let head_exists = git::head_exists(repo)?;
    // With nothing staged the HEAD entry comes back, or none if it is a new file. If something remains
    // staged HEAD's mode also rules: the entry may not exist, in a deletion.
    if result == head {
        commit::unstage(repo, head_exists, &paths)?;
        return Ok(copy);
    }
    let in_head = commit::left_entries(repo, head_exists, &paths)?;
    let saved = if in_head[path] == Saved::Absent {
        commit::read_entries(repo, &paths)?
    } else {
        in_head
    };
    set_index(repo, path, result, &saved)?;
    Ok(copy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hunks::{compute, Hunk};
    use crate::test_support::{RepoHarness, TempRepo};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use tempfile::TempDir;

    const NINE: &[u8] = b"1\n2\n3\n4\n5\n6\n7\n8\n9\n";
    const NINE_EDITED: &[u8] = b"1\ntwo\n3\n4\n5\n6\n7\neight\n9\n";
    const FIRST_ONLY: &str = "1\ntwo\n3\n4\n5\n6\n7\n8\n9\n";

    fn write(repo: &TempRepo, path: &str, contents: &[u8]) {
        let target = repo.path().join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, contents).unwrap();
    }

    fn strings(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|path| path.to_string()).collect()
    }

    fn whole(hunk: &Hunk) -> LineSelection {
        LineSelection {
            hunk: hunk.id.clone(),
            lines: None,
        }
    }

    fn unstaged(repo: &TempRepo, path: &str) -> Vec<Hunk> {
        let (staged, sides) = unstaged_view(repo.path(), path).unwrap().unwrap();
        compute(&staged, &sides.stored)
    }

    fn staged(repo: &TempRepo, path: &str) -> Vec<Hunk> {
        let (head, staged) = staged_view(repo.path(), path).unwrap().unwrap();
        compute(&head, &staged)
    }

    fn index_of(repo: &TempRepo, path: &str) -> String {
        repo.git(&["show", &format!(":{path}")])
    }

    fn nine(repo: &TempRepo) {
        repo.commit("a.txt", NINE, "base");
        write(repo, "a.txt", NINE_EDITED);
    }

    #[test]
    fn stage_of_a_hunk() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        nine(&repo);
        let hunks = unstaged(&repo, "a.txt");
        assert_eq!(hunks.len(), 2);

        let copy = stage_hunks(repo.path(), dir.path(), "a.txt", &[whole(&hunks[0])]).unwrap();

        assert_eq!(index_of(&repo, "a.txt"), FIRST_ONLY);
        assert_eq!(fs::read(repo.path().join("a.txt")).unwrap(), NINE_EDITED);
        assert_eq!(repo.git(&["status", "--porcelain"]), "MM a.txt\n");
        assert_eq!(copy, None);
    }

    #[test]
    fn unstage_of_a_hunk() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        nine(&repo);
        repo.git(&["add", "a.txt"]);
        let hunks = staged(&repo, "a.txt");
        assert_eq!(hunks.len(), 2);

        let copy = unstage_hunks(repo.path(), dir.path(), "a.txt", &[whole(&hunks[1])]).unwrap();

        assert_eq!(index_of(&repo, "a.txt"), FIRST_ONLY);
        assert_eq!(fs::read(repo.path().join("a.txt")).unwrap(), NINE_EDITED);
        assert_eq!(copy, None);
    }

    #[test]
    fn stage_hunks_start_from_the_index_and_not_from_head() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        nine(&repo);
        let hunks = unstaged(&repo, "a.txt");
        stage_hunks(repo.path(), dir.path(), "a.txt", &[whole(&hunks[0])]).unwrap();

        let left = unstaged(&repo, "a.txt");
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].old_start, 7);
        assert_eq!(staged(&repo, "a.txt").len(), 1);

        stage_hunks(repo.path(), dir.path(), "a.txt", &[whole(&left[0])]).unwrap();

        assert_eq!(repo.git(&["status", "--porcelain"]), "M  a.txt\n");
    }

    #[test]
    fn stage_of_a_line_of_a_hunk() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1\n2\n3\n", "base");
        write(&repo, "a.txt", b"1\ntwo\ntwo and a half\n3\n");
        let hunks = unstaged(&repo, "a.txt");
        // Deleting "2" is line 0; adding "two", line 1; adding "two and a half", line 2.
        let line = LineSelection {
            hunk: hunks[0].id.clone(),
            lines: Some(vec![0, 1]),
        };

        stage_hunks(repo.path(), dir.path(), "a.txt", &[line]).unwrap();

        assert_eq!(index_of(&repo, "a.txt"), "1\ntwo\n3\n");
    }

    #[test]
    fn unstage_of_a_line_of_a_hunk() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1\n2\n3\n", "base");
        write(&repo, "a.txt", b"1\n2\nnew\nother\n3\n");
        repo.git(&["add", "a.txt"]);
        let hunks = staged(&repo, "a.txt");
        let line = LineSelection {
            hunk: hunks[0].id.clone(),
            lines: Some(vec![1]),
        };

        unstage_hunks(repo.path(), dir.path(), "a.txt", &[line]).unwrap();

        assert_eq!(index_of(&repo, "a.txt"), "1\n2\nnew\n3\n");
    }

    #[test]
    fn the_entry_mode_is_kept() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("run.sh", NINE, "base");
        fs::set_permissions(
            repo.path().join("run.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        repo.git(&["add", "run.sh"]);
        repo.git(&["commit", "-q", "-m", "executable"]);
        write(&repo, "run.sh", NINE_EDITED);
        let hunks = unstaged(&repo, "run.sh");

        stage_hunks(repo.path(), dir.path(), "run.sh", &[whole(&hunks[0])]).unwrap();

        assert!(repo
            .git(&["ls-files", "-s", "run.sh"])
            .starts_with("100755 "));
    }

    #[test]
    fn partial_stage_of_a_new_file_takes_the_mode_from_the_disk() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1\n", "base");
        write(&repo, "new.sh", b"one\ntwo\n");
        fs::set_permissions(
            repo.path().join("new.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let hunks = unstaged(&repo, "new.sh");
        let line = LineSelection {
            hunk: hunks[0].id.clone(),
            lines: Some(vec![0]),
        };

        stage_hunks(repo.path(), dir.path(), "new.sh", &[line]).unwrap();

        assert_eq!(index_of(&repo, "new.sh"), "one\n");
        assert!(repo
            .git(&["ls-files", "-s", "new.sh"])
            .starts_with("100755 "));
    }

    #[test]
    fn the_result_goes_in_the_stored_form_without_filtering_again() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.git(&["config", "filter.upper.clean", "tr a-z A-Z"]);
        fs::write(repo.path().join(".gitattributes"), "*.txt filter=upper\n").unwrap();
        repo.commit(
            "a.txt",
            b"ONE\nTWO\nTHREE\nFOUR\nFIVE\nSIX\nSEVEN\n",
            "base",
        );
        // On disk, in lowercase: only two lines really change from what git would store.
        write(&repo, "a.txt", b"one\nnew\nthree\nfour\nfive\nsix\nother\n");
        let hunks = unstaged(&repo, "a.txt");
        assert_eq!(hunks.len(), 2);

        stage_hunks(repo.path(), dir.path(), "a.txt", &[whole(&hunks[0])]).unwrap();

        assert_eq!(
            index_of(&repo, "a.txt"),
            "ONE\nNEW\nTHREE\nFOUR\nFIVE\nSIX\nSEVEN\n"
        );
    }

    #[test]
    fn with_autocrlf_input_the_disk_line_endings_do_not_count_as_a_change() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.git(&["config", "core.autocrlf", "input"]);
        repo.commit("a.txt", NINE, "base");
        write(
            &repo,
            "a.txt",
            b"1\r\ntwo\r\n3\r\n4\r\n5\r\n6\r\n7\r\neight\r\n9\r\n",
        );
        let hunks = unstaged(&repo, "a.txt");
        assert_eq!(hunks.len(), 2);

        stage_hunks(repo.path(), dir.path(), "a.txt", &[whole(&hunks[1])]).unwrap();

        assert_eq!(index_of(&repo, "a.txt"), "1\n2\n3\n4\n5\n6\n7\neight\n9\n");
    }

    #[test]
    fn unstage_of_a_whole_new_file_takes_it_out_of_the_index() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1\n", "base");
        write(&repo, "new.txt", b"one\n");
        repo.git(&["add", "new.txt"]);
        let hunks = staged(&repo, "new.txt");

        unstage_hunks(repo.path(), dir.path(), "new.txt", &[whole(&hunks[0])]).unwrap();

        assert_eq!(repo.git(&["status", "--porcelain"]), "?? new.txt\n");
    }

    #[test]
    fn unstage_of_all_hunks_returns_the_head_entry() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        nine(&repo);
        repo.git(&["add", "a.txt"]);
        let selection: Vec<LineSelection> = staged(&repo, "a.txt").iter().map(whole).collect();

        unstage_hunks(repo.path(), dir.path(), "a.txt", &selection).unwrap();

        assert_eq!(repo.git(&["status", "--porcelain"]), " M a.txt\n");
    }

    #[test]
    fn unstage_of_hunks_with_the_file_deleted_on_disk() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        nine(&repo);
        repo.git(&["add", "a.txt"]);
        fs::remove_file(repo.path().join("a.txt")).unwrap();
        let hunks = staged(&repo, "a.txt");

        unstage_hunks(repo.path(), dir.path(), "a.txt", &[whole(&hunks[1])]).unwrap();

        assert_eq!(index_of(&repo, "a.txt"), FIRST_ONLY);
    }

    #[test]
    fn a_hunk_that_no_longer_exists_does_not_touch_the_index() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        nine(&repo);
        let hunks = unstaged(&repo, "a.txt");
        write(&repo, "a.txt", b"1\nanother\n3\n4\n5\n6\n7\n8\n9\n");

        let result = stage_hunks(repo.path(), dir.path(), "a.txt", &[whole(&hunks[1])]);

        assert!(matches!(result, Err(HunkError::Lost { .. })));
        assert_eq!(repo.git(&["status", "--porcelain"]), " M a.txt\n");
    }

    #[test]
    fn with_nothing_chosen_nothing_is_done() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        nine(&repo);
        let hunks = unstaged(&repo, "a.txt");
        let nothing = LineSelection {
            hunk: hunks[0].id.clone(),
            lines: Some(Vec::new()),
        };

        let result = stage_hunks(repo.path(), dir.path(), "a.txt", &[nothing]);

        assert!(matches!(result, Err(HunkError::NothingSelected)));
    }

    #[test]
    fn a_binary_is_not_split() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.bin", b"a\0b\n", "base");
        write(&repo, "a.bin", b"a\0c\n");
        repo.git(&["add", "a.bin"]);

        assert!(staged_view(repo.path(), "a.bin").unwrap().is_none());
        assert!(unstaged_view(repo.path(), "a.bin").unwrap().is_none());
        let result = unstage_hunks(repo.path(), dir.path(), "a.bin", &[]);
        assert!(matches!(result, Err(HunkError::NotSplittable(_))));
    }

    #[test]
    fn a_path_outside_the_repository_is_rejected() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1\n", "base");

        let result = stage_hunks(repo.path(), dir.path(), "../a.txt", &[]);
        assert!(matches!(result, Err(HunkError::OutsideRepository(_))));
        let result = stage(repo.path(), dir.path(), &strings(&["../a.txt"]));
        assert!(matches!(result, Err(HunkError::OutsideRepository(_))));
    }

    fn conflict(repo: &TempRepo) {
        repo.commit("a.txt", b"base\n", "base");
        repo.git(&["checkout", "-q", "-b", "other"]);
        repo.commit("a.txt", b"other\n", "on other");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("a.txt", b"main\n", "on main");
        let merge = Command::new("git")
            .current_dir(repo.path())
            .args(["merge", "-q", "other"])
            .output()
            .unwrap();
        assert!(!merge.status.success());
    }

    #[test]
    fn with_a_conflict_the_index_is_not_touched() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        conflict(&repo);
        let paths = strings(&["a.txt"]);
        let before = repo.git(&["ls-files", "-s"]);

        let results = [
            stage(repo.path(), dir.path(), &paths),
            unstage(repo.path(), dir.path(), &paths),
            stage_hunks(repo.path(), dir.path(), "a.txt", &[]),
            unstage_hunks(repo.path(), dir.path(), "a.txt", &[]),
        ];

        for result in results {
            assert!(matches!(result, Err(HunkError::Unmerged(_))), "{result:?}");
        }
        assert_eq!(repo.git(&["ls-files", "-s"]), before);
    }

    #[test]
    fn stage_of_whole_files() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1\n", "one");
        repo.commit("b.txt", b"1\n", "two");
        write(&repo, "a.txt", b"2\n");
        fs::remove_file(repo.path().join("b.txt")).unwrap();
        write(&repo, "folder/new.txt", b"3\n");

        let copy = stage(
            repo.path(),
            dir.path(),
            &strings(&["a.txt", "b.txt", "folder/new.txt"]),
        )
        .unwrap();

        assert_eq!(
            repo.git(&["status", "--porcelain"]),
            "M  a.txt\nD  b.txt\nA  folder/new.txt\n"
        );
        assert_eq!(copy, None);
    }

    #[test]
    fn unstage_of_whole_files() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1\n", "one");
        write(&repo, "a.txt", b"2\n");
        write(&repo, "new.txt", b"3\n");
        repo.git(&["add", "-A"]);

        unstage(repo.path(), dir.path(), &strings(&["a.txt", "new.txt"])).unwrap();

        assert_eq!(
            repo.git(&["status", "--porcelain"]),
            " M a.txt\n?? new.txt\n"
        );
    }

    #[test]
    fn unstage_on_a_branch_without_commits() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        write(&repo, "a.txt", b"1\n");
        repo.git(&["add", "a.txt"]);

        unstage(repo.path(), dir.path(), &strings(&["a.txt"])).unwrap();

        assert_eq!(repo.git(&["status", "--porcelain"]), "?? a.txt\n");
    }

    // In stage there is something that is neither in HEAD nor on disk.
    fn only_staged(repo: &TempRepo) {
        repo.commit("a.txt", NINE, "base");
        write(repo, "a.txt", b"1\nin stage\n3\n4\n5\n6\n7\n8\n9\n");
        repo.git(&["add", "a.txt"]);
        write(repo, "a.txt", b"1\non disk\n3\n4\n5\n6\n7\n8\n9\n");
    }

    fn assert_copied(dir: &TempDir, id: Option<String>) {
        let id = id.expect("there must be a snapshot");
        let snapshot = snapshots::list(dir.path())
            .into_iter()
            .find(|snapshot| snapshot.id == id)
            .unwrap();
        assert!(snapshot.entries[0].staged_blob);
        let saved = snapshots::stored(dir.path(), &id, "a.txt", true).unwrap();
        assert!(fs::read_to_string(saved).unwrap().contains("in stage"));
    }

    #[test]
    fn stage_of_a_hunk_saves_a_snapshot_of_what_was_only_in_stage() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        only_staged(&repo);
        let hunks = unstaged(&repo, "a.txt");

        let copy = stage_hunks(repo.path(), dir.path(), "a.txt", &[whole(&hunks[0])]).unwrap();

        assert_copied(&dir, copy);
    }

    #[test]
    fn unstage_of_a_hunk_saves_a_snapshot_of_what_was_only_in_stage() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        only_staged(&repo);
        let hunks = staged(&repo, "a.txt");

        let copy = unstage_hunks(repo.path(), dir.path(), "a.txt", &[whole(&hunks[0])]).unwrap();

        assert_copied(&dir, copy);
    }

    #[test]
    fn whole_stage_saves_a_snapshot_of_what_was_only_in_stage() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        only_staged(&repo);

        let copy = stage(repo.path(), dir.path(), &strings(&["a.txt"])).unwrap();

        assert_copied(&dir, copy);
        assert!(index_of(&repo, "a.txt").contains("on disk"));
    }

    #[test]
    fn whole_unstage_saves_a_snapshot_of_what_was_only_in_stage() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        only_staged(&repo);

        let copy = unstage(repo.path(), dir.path(), &strings(&["a.txt"])).unwrap();

        assert_copied(&dir, copy);
        assert_eq!(repo.git(&["status", "--porcelain"]), " M a.txt\n");
    }

    #[test]
    fn what_remains_on_disk_needs_no_snapshot() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        nine(&repo);
        let hunks = unstaged(&repo, "a.txt");
        stage_hunks(repo.path(), dir.path(), "a.txt", &[whole(&hunks[0])]).unwrap();

        let staged_again = staged(&repo, "a.txt");
        let copy =
            unstage_hunks(repo.path(), dir.path(), "a.txt", &[whole(&staged_again[0])]).unwrap();
        assert_eq!(copy, None);

        let hunks = unstaged(&repo, "a.txt");
        stage_hunks(repo.path(), dir.path(), "a.txt", &[whole(&hunks[0])]).unwrap();
        let copy = stage(repo.path(), dir.path(), &strings(&["a.txt"])).unwrap();
        assert_eq!(copy, None);
        assert!(snapshots::list(dir.path()).is_empty());
    }

    #[test]
    fn a_repeated_line_that_was_only_in_stage_counts_as_lost() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"a\nb\n", "base");
        write(&repo, "a.txt", b"a\na\nb\n");
        repo.git(&["add", "a.txt"]);
        write(&repo, "a.txt", b"a\nb\n");
        let hunks = staged(&repo, "a.txt");

        let copy = unstage_hunks(repo.path(), dir.path(), "a.txt", &[whole(&hunks[0])]).unwrap();

        assert!(copy.is_some());
    }

    fn swapped(repo: &TempRepo) {
        repo.commit("a.txt", b"a\nb\n", "base");
        write(repo, "a.txt", b"b\na\n");
        repo.git(&["add", "a.txt"]);
        write(repo, "a.txt", b"a\nb\nc\n");
    }

    #[test]
    fn whole_stage_saves_a_snapshot_of_an_order_that_was_only_in_stage() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        swapped(&repo);

        let copy = stage(repo.path(), dir.path(), &strings(&["a.txt"])).unwrap();

        assert!(copy.is_some());
    }

    #[test]
    fn whole_unstage_saves_a_snapshot_of_an_order_that_was_only_in_stage() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        swapped(&repo);

        let copy = unstage(repo.path(), dir.path(), &strings(&["a.txt"])).unwrap();

        assert!(copy.is_some());
    }

    fn in_sub(sub: &std::path::Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(sub)
            .args(["-c", "user.name=P", "-c", "user.email=p@example.com"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn sub_commit(sub: &std::path::Path, text: &str) -> String {
        fs::write(sub.join("f"), text).unwrap();
        in_sub(sub, &["add", "f"]);
        in_sub(sub, &["commit", "-q", "-m", text]);
        in_sub(sub, &["rev-parse", "HEAD"])
    }

    // A submodule with one commit in HEAD, another in the index and another in its folder.
    fn three_pointers(repo: &TempRepo) {
        repo.commit("a.txt", b"1\n", "base");
        let sub = repo.path().join("sub");
        fs::create_dir(&sub).unwrap();
        in_sub(&sub, &["init", "-q"]);
        let first = sub_commit(&sub, "one");
        repo.git(&[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{first},sub"),
        ]);
        repo.git(&["commit", "-q", "-m", "submodule"]);
        let second = sub_commit(&sub, "two");
        repo.git(&[
            "update-index",
            "--cacheinfo",
            &format!("160000,{second},sub"),
        ]);
        sub_commit(&sub, "three");
        write(repo, "a.txt", b"2\n");
    }

    #[test]
    fn stage_with_a_half_done_submodule_does_not_fail() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        three_pointers(&repo);

        stage(repo.path(), dir.path(), &strings(&["a.txt", "sub"])).unwrap();

        assert_eq!(repo.git(&["status", "--porcelain", "a.txt"]), "M  a.txt\n");
    }

    #[test]
    fn unstage_of_a_half_done_submodule_does_not_fail() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        three_pointers(&repo);

        unstage(repo.path(), dir.path(), &strings(&["sub"])).unwrap();

        assert_eq!(repo.git(&["diff", "--cached", "--name-only"]), "");
    }

    fn file_turned_directory(repo: &TempRepo) {
        repo.commit("a.txt", b"1\n", "base");
        write(repo, "a.txt", b"in stage\n");
        repo.git(&["add", "a.txt"]);
        fs::remove_file(repo.path().join("a.txt")).unwrap();
        write(repo, "a.txt/inside", b"2\n");
    }

    fn assert_index_copy(dir: &TempDir, copy: Option<String>, path: &str, text: &str) {
        let id = copy.expect("there must be a snapshot");
        let entry = snapshots::list(dir.path())
            .into_iter()
            .find(|snapshot| snapshot.id == id)
            .unwrap()
            .entries
            .into_iter()
            .find(|entry| entry.path == path)
            .unwrap();
        assert_eq!(entry.kind, snapshots::EntryKind::Absent);
        assert!(entry.staged_blob);
        let saved = snapshots::stored(dir.path(), &id, path, true).unwrap();
        assert_eq!(fs::read_to_string(saved).unwrap(), text);
    }

    #[test]
    fn a_file_that_is_now_a_folder_does_not_prevent_the_stage() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        file_turned_directory(&repo);

        let copy = stage(repo.path(), dir.path(), &strings(&["a.txt"])).unwrap();

        assert_index_copy(&dir, copy, "a.txt", "in stage\n");
    }

    #[test]
    fn a_file_that_is_now_a_folder_does_not_prevent_the_unstage() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        file_turned_directory(&repo);

        let copy = unstage(repo.path(), dir.path(), &strings(&["a.txt"])).unwrap();

        assert_index_copy(&dir, copy, "a.txt", "in stage\n");
        assert!(repo.path().join("a.txt/inside").exists());
    }

    #[test]
    fn unstage_of_hunks_of_a_file_that_is_now_a_folder() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", NINE, "base");
        write(&repo, "a.txt", NINE_EDITED);
        repo.git(&["add", "a.txt"]);
        fs::remove_file(repo.path().join("a.txt")).unwrap();
        write(&repo, "a.txt/inside", b"2\n");
        let hunks = staged(&repo, "a.txt");

        let copy = unstage_hunks(repo.path(), dir.path(), "a.txt", &[whole(&hunks[1])]).unwrap();

        assert_index_copy(
            &dir,
            copy,
            "a.txt",
            std::str::from_utf8(NINE_EDITED).unwrap(),
        );
        assert_eq!(index_of(&repo, "a.txt"), FIRST_ONLY);
    }

    #[test]
    fn reverting_does_not_delete_a_folder_that_occupies_the_place_of_the_file() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1\n", "base");
        fs::remove_file(repo.path().join("a.txt")).unwrap();
        write(&repo, "a.txt/inside", b"unversioned\n");

        let result = revert(repo.path(), dir.path(), &strings(&["a.txt"]));

        assert!(matches!(result, Err(HunkError::Displaced(_))), "{result:?}");
        assert_eq!(
            fs::read(repo.path().join("a.txt/inside")).unwrap(),
            b"unversioned\n"
        );
        assert!(snapshots::list(dir.path()).is_empty());
    }

    #[test]
    fn reverting_checks_all_paths_before_writing() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1\n", "one");
        repo.commit("b.txt", b"1\n", "two");
        write(&repo, "a.txt", b"edited\n");
        fs::remove_file(repo.path().join("b.txt")).unwrap();
        write(&repo, "b.txt/inside", b"2\n");

        let result = revert(repo.path(), dir.path(), &strings(&["a.txt", "b.txt"]));

        assert!(matches!(result, Err(HunkError::Displaced(_))), "{result:?}");
        assert_eq!(fs::read(repo.path().join("a.txt")).unwrap(), b"edited\n");
    }

    // `d/f` in stage and `d` is now a file.
    fn directory_turned_file(repo: &TempRepo) {
        repo.commit("a.txt", b"1\n", "one");
        repo.commit("d/f", b"1\n", "two");
        write(repo, "d/f", b"in stage\n");
        repo.git(&["add", "d/f"]);
        fs::remove_dir_all(repo.path().join("d")).unwrap();
        write(repo, "d", b"now a file\n");
        write(repo, "a.txt", b"2\n");
    }

    #[test]
    fn a_folder_that_is_now_a_file_does_not_stop_the_stage_of_the_others() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        directory_turned_file(&repo);

        let copy = stage(repo.path(), dir.path(), &strings(&["a.txt", "d/f"])).unwrap();

        assert_index_copy(&dir, copy, "d/f", "in stage\n");
        assert_eq!(index_of(&repo, "a.txt"), "2\n");
    }

    #[test]
    fn a_folder_that_is_now_a_file_does_not_stop_the_unstage_of_the_others() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        directory_turned_file(&repo);
        repo.git(&["add", "a.txt"]);

        let copy = unstage(repo.path(), dir.path(), &strings(&["a.txt", "d/f"])).unwrap();

        assert_index_copy(&dir, copy, "d/f", "in stage\n");
        assert_eq!(index_of(&repo, "a.txt"), "1\n");
    }

    #[test]
    fn reverting_with_a_folder_that_is_now_a_file_is_rejected() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        directory_turned_file(&repo);

        let result = revert(repo.path(), dir.path(), &strings(&["d/f"]));

        let error = result.unwrap_err();
        assert!(matches!(error, HunkError::Displaced(_)), "{error:?}");
        assert!(error.to_string().contains("d/f"), "{error}");
        assert_eq!(fs::read(repo.path().join("d")).unwrap(), b"now a file\n");
        assert!(snapshots::list(dir.path()).is_empty());
    }

    #[test]
    fn restoring_after_the_stage_of_a_file_that_is_now_a_folder_is_rejected() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        file_turned_directory(&repo);
        let id = stage(repo.path(), dir.path(), &strings(&["a.txt"]))
            .unwrap()
            .unwrap();
        let index = repo.git(&["ls-files", "-s"]);

        let error = snapshots::restore(dir.path(), repo.path(), &id).unwrap_err();

        assert!(error.to_string().contains("snapshot"), "{error}");
        assert_eq!(repo.git(&["ls-files", "-s"]), index);
        assert_eq!(snapshots::list(dir.path()).len(), 1);
        let saved = snapshots::stored(dir.path(), &id, "a.txt", true).unwrap();
        assert_eq!(fs::read(saved).unwrap(), b"in stage\n");
    }

    #[test]
    fn stage_of_what_displaces_an_entry_saves_a_snapshot_of_it() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        file_turned_directory(&repo);

        let copy = stage(repo.path(), dir.path(), &strings(&["a.txt/inside"])).unwrap();

        assert_index_copy(&dir, copy, "a.txt", "in stage\n");
    }

    #[test]
    fn stage_of_a_file_that_displaces_what_is_in_its_folder_saves_a_snapshot() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        directory_turned_file(&repo);

        let copy = stage(repo.path(), dir.path(), &strings(&["d"])).unwrap();

        assert_index_copy(&dir, copy, "d/f", "in stage\n");
    }

    fn behind_symlink(repo: &TempRepo) {
        repo.commit("d/f", b"1\n", "base");
        repo.commit("a.txt", b"1\n", "another");
        fs::remove_dir_all(repo.path().join("d")).unwrap();
        fs::create_dir(repo.path().join("e")).unwrap();
        std::os::unix::fs::symlink("e", repo.path().join("d")).unwrap();
    }

    #[test]
    fn reverting_does_not_delete_a_link_on_the_files_way() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        behind_symlink(&repo);

        let result = revert(repo.path(), dir.path(), &strings(&["d/f"]));

        assert!(matches!(result, Err(HunkError::Displaced(_))), "{result:?}");
        let meta = fs::symlink_metadata(repo.path().join("d")).unwrap();
        assert!(meta.file_type().is_symlink());
        assert!(snapshots::list(dir.path()).is_empty());
    }

    #[test]
    fn stage_of_a_path_behind_a_link_does_not_stop_the_others() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        behind_symlink(&repo);
        write(&repo, "a.txt", b"2\n");

        stage(repo.path(), dir.path(), &strings(&["a.txt", "d/f"])).unwrap();

        assert_eq!(index_of(&repo, "a.txt"), "2\n");
    }

    #[test]
    fn stage_of_a_path_behind_a_link_with_content_only_in_stage_saves_a_snapshot() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("d/f", b"1\n", "base");
        repo.commit("a.txt", b"1\n", "another");
        write(&repo, "d/f", b"in stage\n");
        repo.git(&["add", "d/f"]);
        fs::remove_dir_all(repo.path().join("d")).unwrap();
        write(&repo, "e/f", b"1\n");
        std::os::unix::fs::symlink("e", repo.path().join("d")).unwrap();
        write(&repo, "a.txt", b"2\n");

        let copy = stage(repo.path(), dir.path(), &strings(&["d/f", "a.txt"])).unwrap();

        assert_index_copy(&dir, copy, "d/f", "in stage\n");
        assert_eq!(repo.git(&["ls-files", "--", "d/f"]), "");
        assert_eq!(index_of(&repo, "a.txt"), "2\n");
    }

    fn executable(repo: &TempRepo) {
        repo.commit("run.sh", NINE, "base");
        let exec = fs::Permissions::from_mode(0o755);
        fs::set_permissions(repo.path().join("run.sh"), exec).unwrap();
        repo.git(&["add", "run.sh"]);
        repo.git(&["commit", "-q", "-m", "executable"]);
    }

    #[test]
    fn partial_unstage_of_a_deletion_keeps_the_head_mode() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        executable(&repo);
        repo.git(&["rm", "-q", "--cached", "run.sh"]);
        fs::remove_file(repo.path().join("run.sh")).unwrap();
        let hunks = staged(&repo, "run.sh");
        let line = LineSelection {
            hunk: hunks[0].id.clone(),
            lines: Some(vec![0]),
        };

        unstage_hunks(repo.path(), dir.path(), "run.sh", &[line]).unwrap();

        assert!(repo
            .git(&["ls-files", "-s", "run.sh"])
            .starts_with("100755 "));
    }

    #[test]
    fn partial_unstage_keeps_the_head_mode() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        executable(&repo);
        write(&repo, "run.sh", NINE_EDITED);
        let plain = fs::Permissions::from_mode(0o644);
        fs::set_permissions(repo.path().join("run.sh"), plain).unwrap();
        repo.git(&["add", "run.sh"]);
        let hunks = staged(&repo, "run.sh");

        unstage_hunks(repo.path(), dir.path(), "run.sh", &[whole(&hunks[0])]).unwrap();

        assert!(repo
            .git(&["ls-files", "-s", "run.sh"])
            .starts_with("100755 "));
    }

    #[test]
    fn reverting_goes_back_to_what_is_in_the_stage_and_not_to_head() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1\n", "base");
        write(&repo, "a.txt", b"in stage\n");
        repo.git(&["add", "a.txt"]);
        write(&repo, "a.txt", b"on disk\n");

        revert(repo.path(), dir.path(), &strings(&["a.txt"])).unwrap();

        assert_eq!(fs::read(repo.path().join("a.txt")).unwrap(), b"in stage\n");
        assert_eq!(repo.git(&["status", "--porcelain"]), "M  a.txt\n");
    }

    #[test]
    fn reverting_always_saves_a_snapshot_that_can_be_restored() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1\n", "base");
        write(&repo, "a.txt", b"on disk\n");

        let copy = revert(repo.path(), dir.path(), &strings(&["a.txt"])).unwrap();

        assert_eq!(fs::read(repo.path().join("a.txt")).unwrap(), b"1\n");
        snapshots::restore(dir.path(), repo.path(), &copy.unwrap()).unwrap();
        assert_eq!(fs::read(repo.path().join("a.txt")).unwrap(), b"on disk\n");
    }

    #[test]
    fn reverting_with_a_conflict_touches_nothing() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        conflict(&repo);
        let disk = fs::read(repo.path().join("a.txt")).unwrap();

        let result = revert(repo.path(), dir.path(), &strings(&["a.txt"]));

        assert!(matches!(result, Err(HunkError::Unmerged(_))), "{result:?}");
        assert_eq!(fs::read(repo.path().join("a.txt")).unwrap(), disk);
        assert!(snapshots::list(dir.path()).is_empty());
    }

    #[test]
    fn reverting_rejects_foreign_paths() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1\n", "base");

        let result = revert(repo.path(), dir.path(), &strings(&["../a.txt"]));

        assert!(matches!(result, Err(HunkError::OutsideRepository(_))));
    }

    #[test]
    fn reverting_a_submodule_does_not_fail_because_of_the_snapshot() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        three_pointers(&repo);

        revert(repo.path(), dir.path(), &strings(&["a.txt", "sub"])).unwrap();

        assert_eq!(fs::read(repo.path().join("a.txt")).unwrap(), b"1\n");
    }

    // `ChangesView.AddUnversioned` with changelists.
    #[test]
    fn add_to_vcs_adds_the_new_file_to_the_index() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"1\n", "base");
        harness.write("new.txt", "new\n");

        let copy = harness
            .run(|repo| add_to_vcs(repo, &strings(&["new.txt"])))
            .unwrap();

        assert_eq!(copy, None);
        assert_eq!(
            harness.git(&["diff", "--cached", "--name-status"]),
            "A\tnew.txt\n"
        );
        assert_eq!(harness.git(&["show", ":new.txt"]), "new\n");
        assert!(harness.ran("add --ignore-errors -A -- new.txt"));
    }

    #[test]
    fn adding_to_vcs_rejects_foreign_paths() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"1\n", "base");

        let result = harness.run(|repo| add_to_vcs(repo, &strings(&["../a.txt"])));

        assert!(result.is_err());
        assert!(!harness.ran("add --ignore-errors -A -- ../a.txt"));
    }

    // Review Focus 4: what `Add to VCS` put in the index is not "pre-staged" content of the commit of
    // another changelist, and it is still added afterwards.
    #[test]
    fn committing_another_changelist_leaves_the_added_file_out() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"1\n", "base");
        harness.write("new.txt", "new\n");
        harness
            .run(|repo| add_to_vcs(repo, &strings(&["new.txt"])))
            .unwrap();
        harness.write("a.txt", "2\n");
        let request = CommitRequest {
            added: strings(&["a.txt"]),
            message: "Other list".to_string(),
            ..Default::default()
        };

        commit::commit(harness.path(), harness.data.path(), &request).unwrap();

        assert_eq!(
            harness.git(&["show", "--name-status", "--format=", "HEAD"]),
            "M\ta.txt\n"
        );
        assert_eq!(
            harness.git(&["diff", "--cached", "--name-status"]),
            "A\tnew.txt\n"
        );
        assert_eq!(harness.git(&["show", ":new.txt"]), "new\n");
        assert_eq!(harness.git(&["status", "--porcelain"]), "A  new.txt\n");
    }
}
