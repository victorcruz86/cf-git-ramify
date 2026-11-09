use std::path::Path;

use serde::Serialize;

use crate::git::{self, GitError};

pub const MAX_DIFF_BYTES: u64 = 5 * 1024 * 1024;
const BINARY_PROBE_BYTES: usize = 8 * 1024;

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Side {
    Text { content: String },
    Binary,
    TooLarge { bytes: u64 },
    Absent,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    pub path: String,
    pub old_path: Option<String>,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct FileSides {
    pub old: Side,
    pub new: Side,
}

pub fn commit_files(repo: &Path, hash: &str) -> Result<Vec<ChangedFile>, GitError> {
    check_hash(hash)?;
    changes_between(repo, first_parent(repo, hash)?.as_deref(), hash)
}

// Without `base`, what a parentless commit added.
pub fn changes_between(
    repo: &Path,
    base: Option<&str>,
    hash: &str,
) -> Result<Vec<ChangedFile>, GitError> {
    let mut args = vec!["diff-tree", "-r", "-z", "-M", "--name-status"];
    match base {
        Some(base) => args.extend([base, hash]),
        None => args.extend(["--root", "--no-commit-id", hash]),
    }
    Ok(parse_name_status(&git::run(repo, &args)?))
}

pub fn parents_of(repo: &Path, hash: &str) -> Result<Vec<String>, GitError> {
    check_hash(hash)?;
    let revision = format!("{hash}^@");
    let output = git::run(repo, &["rev-parse", &revision])?;
    Ok(String::from_utf8_lossy(&output)
        .split_whitespace()
        .map(str::to_string)
        .collect())
}

pub fn file_sides(
    repo: &Path,
    hash: &str,
    path: &str,
    old_path: Option<&str>,
) -> Result<FileSides, GitError> {
    check_hash(hash)?;
    file_sides_from(
        repo,
        first_parent(repo, hash)?.as_deref(),
        hash,
        path,
        old_path,
    )
}

// `no_base`: the old side does not exist, even if `hash` has parents, as in the change that joins several
// commits and starts at one without parents. Without that, and without `base`, it is compared with the first parent.
pub fn sides_against(
    repo: &Path,
    hash: &str,
    path: &str,
    old_path: Option<&str>,
    base: Option<&str>,
    no_base: bool,
) -> Result<FileSides, GitError> {
    match (no_base, base) {
        (true, _) => file_sides_from(repo, None, hash, path, old_path),
        (false, Some(base)) => file_sides_from(repo, Some(base), hash, path, old_path),
        (false, None) => file_sides(repo, hash, path, old_path),
    }
}

// Without `base`, the file did not exist before.
pub fn file_sides_from(
    repo: &Path,
    base: Option<&str>,
    hash: &str,
    path: &str,
    old_path: Option<&str>,
) -> Result<FileSides, GitError> {
    check_hash(hash)?;
    let old = match base {
        Some(base) => {
            check_hash(base)?;
            read_side(repo, base, old_path.unwrap_or(path))?
        }
        None => Side::Absent,
    };
    let new = read_side(repo, hash, path)?;
    Ok(FileSides { old, new })
}

// A null byte in the first chunk is what git uses to treat a file as binary.
pub fn looks_binary(content: &[u8]) -> bool {
    content[..content.len().min(BINARY_PROBE_BYTES)].contains(&0)
}

// A value starting with `-` would reach git as an option.
pub fn check_hash(hash: &str) -> Result<(), GitError> {
    if matches!(hash.len(), 40 | 64) && hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(GitError::Io(format!("invalid hash: {hash}")))
    }
}

fn first_parent(repo: &Path, hash: &str) -> Result<Option<String>, GitError> {
    let revision = format!("{hash}^1");
    match git::run(repo, &["rev-parse", "--verify", "-q", &revision]) {
        Ok(output) => Ok(Some(String::from_utf8_lossy(&output).trim().to_string())),
        Err(GitError::Failed { .. }) => Ok(None),
        Err(error) => Err(error),
    }
}

pub(crate) fn parse_name_status(output: &[u8]) -> Vec<ChangedFile> {
    let mut fields = output
        .split(|byte| *byte == 0)
        .map(|field| String::from_utf8_lossy(field).into_owned());
    let mut files = Vec::new();
    while let Some(status) = fields.next().filter(|status| !status.is_empty()) {
        let first = fields.next().unwrap_or_default();
        let (path, old_path) = if status.starts_with(['R', 'C']) {
            (fields.next().unwrap_or_default(), Some(first))
        } else {
            (first, None)
        };
        files.push(ChangedFile {
            path,
            old_path,
            status,
        });
    }
    files
}

// How `git diff` shows a submodule: its commit is not in this repository.
pub fn gitlink_side(hash: &str) -> Side {
    Side::Text {
        content: format!("Subproject commit {hash}\n"),
    }
}

pub(crate) fn gitlink_at(
    repo: &Path,
    revision: &str,
    path: &str,
) -> Result<Option<String>, GitError> {
    let args = git::exact(&["--literal-pathspecs", "ls-tree", "-z", revision, "--", path]);
    let listed = git::run(repo, &args)?;
    let head = listed
        .split(|byte| *byte == b'\t')
        .next()
        .unwrap_or_default();
    let head = String::from_utf8_lossy(head);
    let fields: Vec<&str> = head.split_whitespace().collect();
    Ok(match fields.as_slice() {
        ["160000", "commit", hash] => Some(hash.to_string()),
        _ => None,
    })
}

// If `path` exists on disk, git says "exists on disk, but not in"; any other failure is a
// damaged or inaccessible object, not a missing file.
pub(crate) fn is_absent(stderr: &str) -> bool {
    ["does not exist in", "but not in", "Not a valid object name"]
        .iter()
        .any(|message| stderr.contains(message))
}

pub fn read_side(repo: &Path, revision: &str, path: &str) -> Result<Side, GitError> {
    let object = format!("{revision}:{path}");
    let bytes = match git::run(repo, &["cat-file", "-s", &object]) {
        Ok(output) => String::from_utf8_lossy(&output)
            .trim()
            .parse()
            .map_err(|_| GitError::Io(format!("invalid size for {path}")))?,
        Err(GitError::Failed { stderr, .. }) if is_absent(&stderr) => return Ok(Side::Absent),
        Err(failure @ GitError::Failed { .. }) => {
            return match gitlink_at(repo, revision, path)? {
                Some(hash) => Ok(gitlink_side(&hash)),
                None => Err(failure),
            }
        }
        Err(error) => return Err(error),
    };
    if bytes > MAX_DIFF_BYTES {
        return Ok(Side::TooLarge { bytes });
    }
    let content = git::run(repo, &["cat-file", "blob", &object])?;
    if looks_binary(&content) {
        return Ok(Side::Binary);
    }
    Ok(Side::Text {
        content: String::from_utf8_lossy(&content).into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempRepo;

    fn side_text(side: &Side) -> &str {
        match side {
            Side::Text { content } => content,
            other => panic!("expected text: {other:?}"),
        }
    }

    #[test]
    fn modified_file() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one", "first");
        let hash = repo.commit("a.txt", b"two", "second");
        let sides = file_sides(repo.path(), &hash, "a.txt", None).unwrap();
        assert_eq!(side_text(&sides.old), "one");
        assert_eq!(side_text(&sides.new), "two");
    }

    #[test]
    fn new_file() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one", "first");
        let hash = repo.commit("b.txt", b"two", "second");
        let sides = file_sides(repo.path(), &hash, "b.txt", None).unwrap();
        assert_eq!(sides.old, Side::Absent);
    }

    #[test]
    fn deleted_file() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one", "first");
        repo.commit("b.txt", b"two", "second");
        repo.git(&["rm", "-q", "a.txt"]);
        repo.git(&["commit", "-q", "-m", "delete"]);
        let hash = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
        let sides = file_sides(repo.path(), &hash, "a.txt", None).unwrap();
        assert_eq!(sides.new, Side::Absent);
    }

    #[test]
    fn binary_file() {
        let repo = TempRepo::new();
        let hash = repo.commit("a.bin", b"ab\0cd", "binary");
        let sides = file_sides(repo.path(), &hash, "a.bin", None).unwrap();
        assert_eq!(sides.new, Side::Binary);
    }

    #[test]
    fn large_file() {
        let repo = TempRepo::new();
        let hash = repo.commit("a.txt", &vec![b'x'; 6 * 1024 * 1024], "large");
        let sides = file_sides(repo.path(), &hash, "a.txt", None).unwrap();
        assert_eq!(sides.new, Side::TooLarge { bytes: 6291456 });
    }

    #[test]
    fn path_with_spaces_and_accents() {
        let repo = TempRepo::new();
        let path = "folder with space/añó.txt";
        let hash = repo.commit(path, b"hello", "accents");

        let files = commit_files(repo.path(), &hash).unwrap();

        assert_eq!(files[0].path, path);
        let sides = file_sides(repo.path(), &hash, path, None).unwrap();
        assert_eq!(side_text(&sides.new), "hello");
    }

    #[test]
    fn initial_commit() {
        let repo = TempRepo::new();
        let hash = repo.commit("a.txt", b"one", "first");
        let files = commit_files(repo.path(), &hash).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].status, "A");
    }

    #[test]
    fn rename_uses_the_previous_path() {
        let repo = TempRepo::new();
        repo.commit("old.txt", b"enough content to detect the change", "first");
        repo.git(&["mv", "old.txt", "new.txt"]);
        repo.git(&["commit", "-q", "-m", "rename"]);
        let hash = repo.git(&["rev-parse", "HEAD"]).trim().to_string();

        let files = commit_files(repo.path(), &hash).unwrap();

        assert_eq!(files[0].path, "new.txt");
        assert_eq!(files[0].old_path.as_deref(), Some("old.txt"));
        let sides =
            file_sides(repo.path(), &hash, "new.txt", files[0].old_path.as_deref()).unwrap();
        assert_ne!(sides.old, Side::Absent);
    }

    #[test]
    fn merge_compares_with_first_parent() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "base");
        repo.git(&["checkout", "-q", "-b", "other"]);
        repo.commit("b.txt", b"2", "on other");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("c.txt", b"3", "on main");
        repo.git(&["merge", "-q", "--no-ff", "-m", "merge", "other"]);
        let hash = repo.git(&["rev-parse", "HEAD"]).trim().to_string();

        let files = commit_files(repo.path(), &hash).unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "b.txt");
    }

    #[test]
    fn sides_against_another_parent() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "base");
        repo.git(&["checkout", "-q", "-b", "other"]);
        let other = repo.commit("a.txt", b"other", "on other");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("b.txt", b"2", "on main");
        repo.git(&["merge", "-q", "--no-ff", "-m", "merge", "other"]);
        let hash = repo.git(&["rev-parse", "HEAD"]).trim().to_string();

        let sides = file_sides_from(repo.path(), Some(&other), &hash, "a.txt", None).unwrap();

        assert_eq!(side_text(&sides.old), "other");
        assert_eq!(parents_of(repo.path(), &hash).unwrap().len(), 2);
    }

    #[test]
    fn a_parentless_commit_has_no_parents() {
        let repo = TempRepo::new();
        let hash = repo.commit("a.txt", b"1", "base");

        assert!(parents_of(repo.path(), &hash).unwrap().is_empty());
        let sides = file_sides_from(repo.path(), None, &hash, "a.txt", None).unwrap();
        assert_eq!(sides.old, Side::Absent);
    }

    #[test]
    fn deliberately_no_base_does_not_look_at_the_first_parent() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "base");
        let hash = repo.commit("a.txt", b"2", "two");

        let sides = sides_against(repo.path(), &hash, "a.txt", None, None, true).unwrap();
        assert_eq!(sides.old, Side::Absent);

        let sides = sides_against(repo.path(), &hash, "a.txt", None, None, false).unwrap();
        assert_eq!(side_text(&sides.old), "1");
    }

    #[test]
    fn the_base_is_also_checked() {
        let repo = TempRepo::new();
        let hash = repo.commit("a.txt", b"1", "base");

        assert!(file_sides_from(repo.path(), Some("--help"), &hash, "a.txt", None).is_err());
    }

    #[test]
    fn gitlink_is_not_absent() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one", "first");
        let target = "1111111111111111111111111111111111111111";
        repo.git(&[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{target},sub"),
        ]);
        repo.git(&["commit", "-q", "-m", "submodule"]);
        let hash = repo.git(&["rev-parse", "HEAD"]).trim().to_string();

        let sides = file_sides(repo.path(), &hash, "sub", None).unwrap();

        assert_eq!(
            side_text(&sides.new),
            format!("Subproject commit {target}\n")
        );
        assert_eq!(sides.old, Side::Absent);
    }

    #[test]
    fn damaged_object_gives_error() {
        let repo = TempRepo::new();
        let hash = repo.commit("a.txt", b"one", "first");
        let blob = repo.git(&["rev-parse", "HEAD:a.txt"]);
        let blob = blob.trim();
        let loose = repo
            .path()
            .join(".git/objects")
            .join(&blob[..2])
            .join(&blob[2..]);
        std::fs::remove_file(loose).unwrap();

        assert!(file_sides(repo.path(), &hash, "a.txt", None).is_err());
    }

    #[test]
    fn hash_that_looks_like_an_option_is_rejected() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one", "first");
        assert!(commit_files(repo.path(), "--output=/tmp/x").is_err());
        assert!(file_sides(repo.path(), "--help", "a.txt", None).is_err());
    }
}
