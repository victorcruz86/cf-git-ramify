use std::path::Path;

use serde::Serialize;

use crate::git::{self, GitError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FileState {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    Unmerged,
    Untracked,
    Ignored,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub path: String,
    pub old_path: Option<String>,
    pub state: FileState,
    pub staged: bool,
    pub unstaged: bool,
}

pub fn read(repo: &Path, include_ignored: bool) -> Result<Vec<FileChange>, GitError> {
    let mut args = vec![
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
        "--ignore-submodules=dirty",
    ];
    // `matching`: an ignored folder comes once, as `dir/`, and not file by file (with
    // `--untracked-files=all` the traditional mode lists every file under `node_modules`).
    if include_ignored {
        args.push("--ignored=matching");
    }
    Ok(parse(&git::run(repo, &args)?))
}

// Each entry is `XY path`; for renames and copies the old path comes in the next field.
fn parse(output: &[u8]) -> Vec<FileChange> {
    let mut fields = output
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .map(|field| String::from_utf8_lossy(field).into_owned());
    let mut changes = Vec::new();
    while let Some(entry) = fields.next() {
        let Some((codes, path)) = entry.split_at_checked(3) else {
            continue;
        };
        let mut codes = codes.chars();
        let (Some(x), Some(y)) = (codes.next(), codes.next()) else {
            continue;
        };
        let old_path = if matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C') {
            fields.next()
        } else {
            None
        };
        changes.push(FileChange {
            path: path.to_string(),
            old_path,
            state: state_of(x, y),
            staged: !matches!(x, ' ' | '?' | '!') && !is_unmerged(x, y),
            unstaged: y != '!' && (y != ' ' || is_unmerged(x, y)),
        });
    }
    changes
}

fn is_unmerged(x: char, y: char) -> bool {
    matches!((x, y), ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D'))
}

// If there is a change in the index that one wins; `AD` is a new file even though it no longer exists on disk.
fn state_of(x: char, y: char) -> FileState {
    if is_unmerged(x, y) {
        return FileState::Unmerged;
    }
    match (x, y) {
        ('?', _) => FileState::Untracked,
        ('!', _) => FileState::Ignored,
        _ => match if x == ' ' { y } else { x } {
            'A' => FileState::Added,
            'D' => FileState::Deleted,
            'R' => FileState::Renamed,
            'C' => FileState::Copied,
            _ => FileState::Modified,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempRepo;
    use std::fs;

    fn write(repo: &TempRepo, file: &str, contents: &str) {
        let target = repo.path().join(file);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, contents).unwrap();
    }

    fn only(changes: Vec<FileChange>) -> FileChange {
        assert_eq!(changes.len(), 1, "{changes:?}");
        changes.into_iter().next().unwrap()
    }

    #[test]
    fn modified_unstaged() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        write(&repo, "a.txt", "2");

        let change = only(read(repo.path(), false).unwrap());

        assert_eq!(change.path, "a.txt");
        assert_eq!(change.state, FileState::Modified);
        assert!(change.unstaged);
        assert!(!change.staged);
    }

    #[test]
    fn modified_staged_and_unstaged() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        write(&repo, "a.txt", "2");
        repo.git(&["add", "a.txt"]);
        write(&repo, "a.txt", "3");

        let change = only(read(repo.path(), false).unwrap());

        assert_eq!(change.state, FileState::Modified);
        assert!(change.staged);
        assert!(change.unstaged);
    }

    #[test]
    fn unversioned_file() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        write(&repo, "new.txt", "x");

        let change = only(read(repo.path(), false).unwrap());

        assert_eq!(change.path, "new.txt");
        assert_eq!(change.state, FileState::Untracked);
        assert!(change.unstaged);
        assert!(!change.staged);
    }

    #[test]
    fn staged_rename() {
        let repo = TempRepo::new();
        repo.commit("old.txt", b"long content to detect the rename\n", "one");
        repo.git(&["mv", "old.txt", "new.txt"]);

        let change = only(read(repo.path(), false).unwrap());

        assert_eq!(change.state, FileState::Renamed);
        assert_eq!(change.path, "new.txt");
        assert_eq!(change.old_path.as_deref(), Some("old.txt"));
        assert!(change.staged);
        assert!(!change.unstaged);
    }

    #[test]
    fn conflict() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"base\n", "base");
        repo.git(&["checkout", "-q", "-b", "other"]);
        repo.commit("a.txt", b"other\n", "other");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("a.txt", b"main\n", "main");
        let merge = crate::test_support::git_command()
            .current_dir(repo.path())
            .args(["merge", "other"])
            .output()
            .unwrap();
        assert!(!merge.status.success());

        let change = only(read(repo.path(), false).unwrap());

        assert_eq!(change.path, "a.txt");
        assert_eq!(change.state, FileState::Unmerged);
    }

    #[test]
    fn deleted_after_staging_again() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        write(&repo, "new.txt", "x");
        repo.git(&["add", "new.txt"]);
        fs::remove_file(repo.path().join("new.txt")).unwrap();

        let change = only(read(repo.path(), false).unwrap());

        assert_eq!(change.path, "new.txt");
        assert_eq!(change.state, FileState::Added);
        assert!(change.staged);
        assert!(change.unstaged);
    }

    #[test]
    fn path_with_spaces_and_accents() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        write(&repo, "folder with space/ñandú \"é\".txt", "x");

        let change = only(read(repo.path(), false).unwrap());

        assert_eq!(change.path, "folder with space/ñandú \"é\".txt");
    }

    #[test]
    fn ignored_only_when_requested() {
        let repo = TempRepo::new();
        repo.commit(".gitignore", b"*.log\n", "ignore");
        write(&repo, "debug.log", "x");

        assert!(read(repo.path(), false).unwrap().is_empty());

        let change = only(read(repo.path(), true).unwrap());
        assert_eq!(change.path, "debug.log");
        assert_eq!(change.state, FileState::Ignored);
        assert!(!change.staged);
        assert!(!change.unstaged);
    }

    #[test]
    fn an_ignored_folder_comes_once_as_a_folder() {
        let repo = TempRepo::new();
        repo.commit(".gitignore", b"build/\n*.o\n", "ignore");
        write(&repo, "build/deep/a.txt", "1");
        write(&repo, "build/b.txt", "2");
        write(&repo, "src/c.o", "3");

        let ignored: Vec<_> = read(repo.path(), true)
            .unwrap()
            .into_iter()
            .map(|change| (change.path, change.state))
            .collect();

        assert_eq!(
            ignored,
            [
                ("build/".to_string(), FileState::Ignored),
                ("src/c.o".to_string(), FileState::Ignored)
            ]
        );
    }

    #[test]
    fn unversioned_folder_lists_its_files() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        write(&repo, "dir/one.txt", "1");
        write(&repo, "dir/sub/two.txt", "2");

        let paths: Vec<_> = read(repo.path(), false)
            .unwrap()
            .into_iter()
            .map(|change| change.path)
            .collect();

        assert_eq!(paths, ["dir/one.txt", "dir/sub/two.txt"]);
    }
}
