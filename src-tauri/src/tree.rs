// The Project tool window's folders, read from disk on expand (decision 3 of phase 4 part 4): the
// entries of one folder, without IntelliJ's default ignore patterns, folders first and then by name
// as `FileNameComparator` orders them. The Repositories tool window's folders, one level of a
// revision read on expand (`GitDirectoryVirtualFile`).

use std::cmp::Ordering;
use std::fs;
use std::io;
use std::path::{Component, Path};

use serde::Serialize;
use tauri::{State, WebviewWindow};

use crate::commands::{blocking_api, current_repo, failed, ApiError, AppState};
use crate::diff::check_hash;
use crate::git::{self, GitError};
use crate::hunks::HunkError;
use crate::viewer;
use crate::worktree_diff;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeEntry {
    pub name: String,
    // Relative to the repository, with `/`, exactly as the disk spells it.
    pub path: String,
    pub dir: bool,
    // A file of a type IntelliJ registers as binary (`viewer::binary_type`): it is not annotated.
    pub binary: bool,
}

impl TreeEntry {
    pub fn new(name: String, path: String, dir: bool) -> Self {
        let binary = !dir && viewer::binary_type(&path);
        Self {
            name,
            path,
            dir,
            binary,
        }
    }
}

#[derive(Debug)]
pub enum TreeError {
    Outside(String),
    Io(io::Error),
    Git(GitError),
}

impl From<TreeError> for ApiError {
    fn from(error: TreeError) -> Self {
        match error {
            TreeError::Outside(path) => failed(format!(
                "The path \"{path}\" is not valid inside the repository"
            )),
            TreeError::Io(error) => failed(error.to_string()),
            TreeError::Git(error) => error.into(),
        }
    }
}

// `FileTypeManagerImpl.DEFAULT_IGNORED`, IntelliJ's "Ignored Files and Folders": the exact names,
// matched with their case as `ExactFileNameMatcher` does, and the extensions of the `*.ext` masks,
// matched without it as `FileTypeAssocTable` does.
const HIDDEN_NAMES: [&str; 12] = [
    ".DS_Store",
    ".git",
    ".hg",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".svn",
    "CVS",
    "__pycache__",
    "_svn",
    "vssver.scc",
    "vssver2.scc",
];
const HIDDEN_EXTENSIONS: [&str; 4] = ["pyc", "pyo", "rbc", "yarb"];
// `FileUtil.ASYNC_DELETE_EXTENSION`, which `IgnoredPatternSet.isIgnored` hides as well.
const ASYNC_DELETE_EXTENSION: &str = ".__del__";

fn is_hidden(name: &str) -> bool {
    HIDDEN_NAMES.contains(&name)
        // Ramify never goes into a `.git`, whatever its case (`snapshots::check_path`).
        || is_git_dir(name)
        // The `*~` mask.
        || name.ends_with('~')
        || name.ends_with(ASYNC_DELETE_EXTENSION)
        || name.rsplit_once('.').is_some_and(|(_, extension)| {
            HIDDEN_EXTENSIONS
                .iter()
                .any(|hidden| hidden.eq_ignore_ascii_case(extension))
        })
}

fn is_git_dir(name: &str) -> bool {
    name.eq_ignore_ascii_case(".git")
}

// `NaturalComparator.naturalCompare(s1, s2, ignoreCase = true, likeFileNames = true)`, which
// `FileNameComparator` gives `AlphaComparator`: numbers by value, `-` as `_`, and the case only
// when nothing else tells the names apart. Java compares UTF-16 units, this compares characters.
fn natural_compare(a: &str, b: &str) -> Ordering {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    compare_names(&a, &b, true, true)
}

// `StringUtil.naturalCompare`, which is `NaturalComparator.INSTANCE`: the same without the rule
// of file names (`likeFileNames = false`), so `-` is itself.
fn plain_natural_compare(a: &str, b: &str) -> Ordering {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    compare_names(&a, &b, true, false)
}

fn compare_names(s1: &[char], s2: &[char], ignore_case: bool, like_file_names: bool) -> Ordering {
    let (length1, length2) = (s1.len(), s2.len());
    let (mut i, mut j) = (0, 0);
    while i < length1 && j < length2 {
        let (ch1, ch2) = (s1[i], s2[j]);
        if is_digit_or_space(ch1) && is_digit_or_space(ch2) {
            let start1 = skip_char(s1, skip_char(s1, i, ' '), '0');
            let start2 = skip_char(s2, skip_char(s2, j, ' '), '0');
            let end1 = skip_digits(s1, start1);
            let end2 = skip_digits(s2, start2);
            // A number with more digits is greater; then the digits; then the number with its
            // leading spaces and zeros, shorter first; then those spaces and zeros.
            let order = (end1 - start1)
                .cmp(&(end2 - start2))
                .then_with(|| s1[start1..end1].cmp(&s2[start2..end2]))
                .then_with(|| (end1 - i).cmp(&(end2 - j)))
                .then_with(|| s1[i..start1].cmp(&s2[j..start2]));
            if order != Ordering::Equal {
                return order;
            }
            i = end1;
            j = end2;
            continue;
        }
        if ch1 != ch2 {
            // For file names, "super natural comparison" (IDEA-80435): `-` sorts as `_`, except
            // against `_`.
            let order = if !like_file_names {
                compare_chars(ch1, ch2, ignore_case)
            } else if ch1 == '-' && ch2 != '_' {
                compare_chars('_', ch2, ignore_case)
            } else if ch2 == '-' && ch1 != '_' {
                compare_chars(ch1, '_', ignore_case)
            } else {
                compare_chars(ch1, ch2, ignore_case)
            };
            if order != Ordering::Equal {
                return order;
            }
        }
        i += 1;
        j += 1;
    }
    if i < length1 {
        return Ordering::Greater;
    }
    if j < length2 {
        return Ordering::Less;
    }
    length1.cmp(&length2).then_with(|| {
        if ignore_case {
            compare_names(s1, s2, false, like_file_names)
        } else {
            Ordering::Equal
        }
    })
}

fn is_digit_or_space(ch: char) -> bool {
    ch.is_ascii_digit() || ch == ' '
}

fn skip_char(s: &[char], mut start: usize, ch: char) -> usize {
    while start < s.len() && s[start] == ch {
        start += 1;
    }
    start
}

fn skip_digits(s: &[char], mut start: usize) -> usize {
    while start < s.len() && s[start].is_ascii_digit() {
        start += 1;
    }
    start
}

// The transitivity fix of `compareChars`: a space sorts after the characters between it and `0`.
fn compare_chars(ch1: char, ch2: char, ignore_case: bool) -> Ordering {
    if ch1 == ' ' && ch2 > ' ' && ch2 < '0' {
        return Ordering::Greater;
    }
    if ch2 == ' ' && ch1 > ' ' && ch1 < '0' {
        return Ordering::Less;
    }
    if ch1 == ch2 || !ignore_case {
        return ch1.cmp(&ch2);
    }
    // `Strings.compare`: as `String.equalsIgnoreCase`, upper case and then lower case.
    let (upper1, upper2) = (
        single(ch1.to_uppercase(), ch1),
        single(ch2.to_uppercase(), ch2),
    );
    if upper1 == upper2 {
        return Ordering::Equal;
    }
    single(upper1.to_lowercase(), upper1).cmp(&single(upper2.to_lowercase(), upper2))
}

// `Character.toUpperCase(char)` maps one character to one: a longer mapping leaves it as it is.
fn single(mut mapped: impl Iterator<Item = char>, original: char) -> char {
    match (mapped.next(), mapped.next()) {
        (Some(only), None) => only,
        _ => original,
    }
}

// Inside the repository and out of any `.git`, once links are resolved.
fn is_inside(root: &Path, real: &Path) -> bool {
    real.strip_prefix(root).is_ok_and(|rest| {
        !rest.components().any(
            |part| matches!(part, Component::Normal(name) if name.to_str().is_some_and(is_git_dir)),
        )
    })
}

// The repository's own path rules (`worktree_diff::check_inside`: relative, no `..`, not into
// `.git`, every link on the way and the folder's own link ending inside), and not a link into `.git`.
fn check_folder(repo: &Path, root: &Path, dir: &str) -> Result<(), TreeError> {
    let outside = || TreeError::Outside(dir.to_string());
    worktree_diff::check_inside(repo, dir).map_err(|error| match error {
        HunkError::OutsideRepository(_) => outside(),
        HunkError::Io(error) => TreeError::Io(error),
        other => TreeError::Io(io::Error::other(other.to_string())),
    })?;
    match fs::canonicalize(repo.join(dir)) {
        Ok(real) if !is_inside(root, &real) => Err(outside()),
        _ => Ok(()),
    }
}

// A folder reached again through a link inside itself (`self -> .`, `a/up -> ..`) is listed empty,
// as IntelliJ's file system does with a recursive link: expanding everything has to end.
fn is_circular(repo: &Path, dir: &str) -> bool {
    let Ok(real) = fs::canonicalize(repo.join(dir)) else {
        return false;
    };
    let mut ancestor = Path::new(dir);
    while let Some(parent) = ancestor.parent() {
        if fs::canonicalize(repo.join(parent)).is_ok_and(|parent| parent == real) {
            return true;
        }
        ancestor = parent;
    }
    false
}

// A folder, or a link to a folder inside the repository. A link that ends outside it (or in `.git`)
// is listed as a file: the tree never lists what is outside, and the viewer refuses to open it.
// The kind comes from the folder read itself; only a link costs another look at the disk.
fn is_folder(root: &Path, item: &fs::DirEntry) -> bool {
    let Ok(kind) = item.file_type() else {
        return false;
    };
    if !kind.is_symlink() {
        return kind.is_dir();
    }
    fs::canonicalize(item.path()).is_ok_and(|real| real.is_dir() && is_inside(root, &real))
}

// The entries of `dir` (relative to the repository, "" for its root), as the disk has them now.
// A folder that is gone, or is no longer a folder, has none.
pub fn list_dir(repo: &Path, dir: &str) -> Result<Vec<TreeEntry>, TreeError> {
    let root = fs::canonicalize(repo).map_err(TreeError::Io)?;
    let dir = dir.trim_end_matches('/');
    if !dir.is_empty() {
        check_folder(repo, &root, dir)?;
        if is_circular(repo, dir) {
            return Ok(Vec::new());
        }
    }
    let reader = match fs::read_dir(repo.join(dir)) {
        Ok(reader) => reader,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(Vec::new())
        }
        Err(error) => return Err(TreeError::Io(error)),
    };
    let mut entries = Vec::new();
    for item in reader {
        // An entry that cannot be read (gone meanwhile) is left out, not the whole folder.
        let Ok(item) = item else {
            continue;
        };
        // macOS names are UTF-8; one that is not could not be named to the other commands.
        let Ok(name) = item.file_name().into_string() else {
            continue;
        };
        if is_hidden(&name) {
            continue;
        }
        let path = if dir.is_empty() {
            name.clone()
        } else {
            format!("{dir}/{name}")
        };
        let dir = is_folder(&root, &item);
        entries.push(TreeEntry::new(name, path, dir));
    }
    // `GroupByTypeComparator` with `Folders Always on Top`, then `AlphaComparator`.
    entries.sort_by(|a, b| {
        b.dir
            .cmp(&a.dir)
            .then_with(|| natural_compare(&a.name, &b.name))
    });
    Ok(entries)
}

// `ArchiveFileType` (`PlatformExtensions.xml:685-686`), matched without case as an extension is.
// `FileChooserDescriptor.isFileVisible` hides an archive from a chooser that takes neither jars
// nor their contents, which the Repositories tree's (`RepositoryBrowserPanel`) is.
const ARCHIVE_EXTENSIONS: [&str; 8] = ["ane", "apk", "ear", "egg", "jar", "swc", "war", "zip"];

fn is_archive(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, extension)| {
        ARCHIVE_EXTENSIONS
            .iter()
            .any(|archive| archive.eq_ignore_ascii_case(extension))
    })
}

// A folder of a revision, relative to the repository ("" for its root): no empty, `.` or `..`
// part, nothing absolute, never into `.git`. Git's own trees have none of those.
fn check_revision_folder(dir: &str) -> Result<(), TreeError> {
    let valid = dir.is_empty()
        || dir
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != ".." && !is_git_dir(part));
    if valid {
        Ok(())
    } else {
        Err(TreeError::Outside(dir.to_string()))
    }
}

// The entries of `dir` at `revision` (a full hash): `GitIndexUtil.listTreeForRawPaths` with
// `git ls-tree <rev> -- <dir>/`, through `git::exact` and with the folder as a literal, so it is
// that folder's exact bytes and no pattern. Without `-r`, one level: a `tree` is a folder, a
// `blob` or a submodule (`commit`) a file. `FileChooserDescriptor.isFileVisible` hides what
// IntelliJ ignores and the archives (not the dot files: those are hidden only on the local disk),
// and `FileTreeModel` lists folders first, then by `StringUtil.naturalCompare`.
pub fn ls_tree(repo: &Path, revision: &str, dir: &str) -> Result<Vec<TreeEntry>, TreeError> {
    check_hash(revision).map_err(TreeError::Git)?;
    let dir = dir.strip_suffix('/').unwrap_or(dir);
    check_revision_folder(dir)?;
    let prefix = if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    };
    let mut args = git::exact(&["--literal-pathspecs", "ls-tree", "-z", revision, "--"]);
    if !prefix.is_empty() {
        args.push(&prefix);
    }
    let listed = git::run(repo, &args).map_err(TreeError::Git)?;
    let mut entries = Vec::new();
    // `<mode> <type> <object>\t<path>` records.
    for record in listed.split(|byte| *byte == 0) {
        let Some(tab) = record.iter().position(|byte| *byte == b'\t') else {
            continue;
        };
        // As in `list_dir`: a name that is not UTF-8 could not be named to the other commands.
        let Ok(path) = std::str::from_utf8(&record[tab + 1..]) else {
            continue;
        };
        let Some(name) = path.strip_prefix(prefix.as_str()) else {
            continue;
        };
        if name.is_empty() || name.contains('/') {
            continue;
        }
        let head = String::from_utf8_lossy(&record[..tab]);
        let dir = match head.split(' ').nth(1) {
            Some("tree") => true,
            Some("blob" | "commit") => false,
            _ => continue,
        };
        if is_hidden(name) || (!dir && is_archive(name)) {
            continue;
        }
        entries.push(TreeEntry::new(name.to_string(), path.to_string(), dir));
    }
    entries.sort_by(|a, b| {
        b.dir
            .cmp(&a.dir)
            .then_with(|| plain_natural_compare(&a.name, &b.name))
    });
    Ok(entries)
}

#[tauri::command]
pub async fn tree_list(
    dir: String,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Vec<TreeEntry>, ApiError> {
    let repo = current_repo(&state, &window)?;
    blocking_api(move || Ok(list_dir(&repo, &dir)?)).await
}

// The Repositories tool window: one folder of `revision`, read when it is expanded.
#[tauri::command]
pub async fn tree_list_at(
    revision: String,
    dir: String,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Vec<TreeEntry>, ApiError> {
    let repo = current_repo(&state, &window)?;
    blocking_api(move || Ok(ls_tree(&repo, &revision, &dir)?)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::{self, FileState};
    use crate::test_support::TempRepo;
    use crate::viewer::{self, ViewerContent, ViewerTarget};
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    fn touch(repo: &TempRepo, path: &str) {
        let full = repo.path().join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, path).unwrap();
    }

    fn names(entries: &[TreeEntry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.name.as_str()).collect()
    }

    #[test]
    fn folders_first_then_natural_case_insensitive() {
        let repo = TempRepo::new();
        for file in [
            "a10.txt", "a2.txt", "b.txt", "B.md", "A1.txt", "my_file", "my-file2",
        ] {
            touch(&repo, file);
        }
        for folder in ["src10/x", "src2/x", "Docs/x", "assets/x"] {
            touch(&repo, folder);
        }

        let entries = list_dir(repo.path(), "").unwrap();

        assert_eq!(
            names(&entries),
            [
                "assets", "Docs", "src2", "src10", "A1.txt", "a2.txt", "a10.txt", "B.md", "b.txt",
                "my-file2", "my_file",
            ]
        );
        let docs = entries.iter().find(|entry| entry.name == "Docs").unwrap();
        assert_eq!(
            docs,
            &TreeEntry::new("Docs".to_string(), "Docs".to_string(), true)
        );
        let nested = list_dir(repo.path(), "src2").unwrap();
        assert_eq!(
            nested,
            [TreeEntry::new("x".to_string(), "src2/x".to_string(), false)]
        );
    }

    #[test]
    fn natural_order_is_the_one_of_file_name_comparator() {
        let mut names = vec![
            "a 2", "a02", "a2", "a_b", "a-c", "a.c", "A", "a", "x#", "x ",
        ];
        names.sort_by(|a, b| natural_compare(a, b));
        // `-` counts as `_` except against `_` itself; a space sorts after `#` (the transitivity fix).
        assert_eq!(
            names,
            ["A", "a", "a.c", "a2", "a 2", "a02", "a-c", "a_b", "x#", "x "]
        );
    }

    #[test]
    fn ds_store_and_dot_git_are_hidden_but_ignored_files_are_shown_coloured() {
        let repo = TempRepo::new();
        repo.commit(".gitignore", b"*.log\nbuild/\n", "ignore");
        for file in [
            ".DS_Store",
            "debug.log",
            "build/out.o",
            "cache.pyc",
            "notes.txt~",
            "__pycache__/m.pyc",
            ".idea/workspace.xml",
            "keep.PYC",
            "ds_store",
        ] {
            touch(&repo, file);
        }

        let entries = list_dir(repo.path(), "").unwrap();

        assert_eq!(
            names(&entries),
            [".idea", "build", ".gitignore", "debug.log", "ds_store"]
        );
        // The tree colours them from `git status --ignored`: an ignored folder comes once, as a folder.
        let ignored: Vec<_> = status::read(repo.path(), true)
            .unwrap()
            .into_iter()
            .filter(|change| change.state == FileState::Ignored)
            .map(|change| change.path)
            .collect();
        assert!(ignored.contains(&"debug.log".to_string()), "{ignored:?}");
        assert!(ignored.contains(&"build/".to_string()), "{ignored:?}");
        assert!(
            !ignored.iter().any(|path| path.starts_with("build/o")),
            "{ignored:?}"
        );
    }

    #[test]
    fn a_name_with_glob_characters_and_a_leading_dash_lists_and_opens() {
        let repo = TempRepo::new();
        let nfd = "cafe\u{301}.txt";
        for file in [
            "[ab]/-x*?.txt",
            "[ab]/a.txt",
            "b/a.txt",
            ":(glob)*",
            "with space.txt",
            nfd,
        ] {
            touch(&repo, file);
        }

        let folder = list_dir(repo.path(), "[ab]").unwrap();
        assert_eq!(
            folder
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            ["[ab]/-x*?.txt", "[ab]/a.txt"]
        );
        let root = list_dir(repo.path(), "").unwrap();
        let root_paths: Vec<_> = root.iter().map(|entry| entry.path.as_str()).collect();
        assert!(root_paths.contains(&":(glob)*"), "{root_paths:?}");
        assert!(
            root_paths.contains(&nfd),
            "the decomposed name as the disk has it: {root_paths:?}"
        );

        for path in ["[ab]/-x*?.txt", ":(glob)*", nfd] {
            let target = ViewerTarget {
                path: path.to_string(),
                revision: None,
                line: None,
                annotate: false,
            };
            match viewer::read(repo.path(), &target) {
                Ok(ViewerContent::Text { text, .. }) => assert_eq!(text, path),
                other => panic!("{path}: {other:?}"),
            }
        }
    }

    #[test]
    fn a_folder_outside_the_repository_is_refused() {
        let repo = TempRepo::new();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("secret.txt"), "x").unwrap();
        touch(&repo, "src/a.txt");
        symlink(outside.path(), repo.path().join("away")).unwrap();
        symlink(repo.path().join(".git"), repo.path().join("into-git")).unwrap();

        for dir in [
            "..",
            "src/../..",
            "/etc",
            outside.path().to_str().unwrap(),
            ".git",
            ".git/refs",
            "away",
            "into-git",
        ] {
            assert!(
                matches!(list_dir(repo.path(), dir), Err(TreeError::Outside(_))),
                "{dir}"
            );
        }
    }

    #[test]
    fn a_link_out_of_the_repository_is_listed_but_never_followed() {
        let repo = TempRepo::new();
        let outside = TempDir::new().unwrap();
        fs::create_dir(outside.path().join("inner")).unwrap();
        touch(&repo, "src/a.txt");
        symlink(outside.path(), repo.path().join("away")).unwrap();
        symlink(repo.path().join("src"), repo.path().join("inside")).unwrap();
        symlink(repo.path().join("missing"), repo.path().join("broken")).unwrap();

        let entries = list_dir(repo.path(), "").unwrap();

        let kinds: Vec<_> = entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.dir))
            .collect();
        assert_eq!(
            kinds,
            [
                ("inside", true),
                ("src", true),
                ("away", false),
                ("broken", false)
            ]
        );
        assert_eq!(
            list_dir(repo.path(), "inside").unwrap(),
            [TreeEntry::new(
                "a.txt".to_string(),
                "inside/a.txt".to_string(),
                false
            )]
        );
    }

    #[test]
    fn a_link_out_of_the_repository_listed_as_a_file_does_not_open_its_target() {
        let repo = TempRepo::new();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("secret.txt"), "secret").unwrap();
        symlink(
            outside.path().join("secret.txt"),
            repo.path().join("away.txt"),
        )
        .unwrap();
        symlink(outside.path(), repo.path().join("away")).unwrap();

        let entries = list_dir(repo.path(), "").unwrap();
        assert!(entries.iter().all(|entry| !entry.dir), "{entries:?}");

        // `viewer_open` keeps a target only after `check_inside`, and `viewer::read` checks it again.
        for path in ["away.txt", "away/secret.txt"] {
            assert!(
                worktree_diff::check_inside(repo.path(), path).is_err(),
                "{path}"
            );
            let target = ViewerTarget {
                path: path.to_string(),
                revision: None,
                line: None,
                annotate: false,
            };
            assert!(viewer::read(repo.path(), &target).is_err(), "{path}");
        }
    }

    #[test]
    fn a_link_back_into_its_own_folder_lists_nothing() {
        let repo = TempRepo::new();
        touch(&repo, "src/a/b.txt");
        symlink(repo.path(), repo.path().join("self")).unwrap();
        symlink("..", repo.path().join("src/a/up")).unwrap();

        assert!(list_dir(repo.path(), "").unwrap().contains(&TreeEntry::new(
            "self".to_string(),
            "self".to_string(),
            true
        )));
        assert_eq!(list_dir(repo.path(), "self").unwrap(), []);
        assert_eq!(list_dir(repo.path(), "src/a/up").unwrap(), []);
        assert_eq!(
            names(&list_dir(repo.path(), "src/a").unwrap()),
            ["up", "b.txt"]
        );
    }

    #[test]
    fn a_folder_that_is_gone_lists_nothing() {
        let repo = TempRepo::new();
        touch(&repo, "a.txt");

        assert_eq!(list_dir(repo.path(), "gone").unwrap(), []);
        assert_eq!(list_dir(repo.path(), "a.txt").unwrap(), []);
    }

    #[test]
    fn a_change_on_disk_recolours_and_relists_open_folders() {
        let repo = TempRepo::new();
        repo.commit("src/a.txt", b"1", "one");
        assert_eq!(names(&list_dir(repo.path(), "src").unwrap()), ["a.txt"]);

        touch(&repo, "src/b.txt");
        fs::write(repo.path().join("src/a.txt"), "2").unwrap();

        assert_eq!(
            names(&list_dir(repo.path(), "src").unwrap()),
            ["a.txt", "b.txt"]
        );
        let states: Vec<_> = status::read(repo.path(), true)
            .unwrap()
            .into_iter()
            .map(|change| (change.path, change.state))
            .collect();
        assert_eq!(
            states,
            [
                ("src/a.txt".to_string(), FileState::Modified),
                ("src/b.txt".to_string(), FileState::Untracked)
            ]
        );
    }

    // Files straight into the index under their exact names (`git add` would skip what a global
    // excludes file ignores, and precompose a decomposed name), then one commit.
    fn commit_exact(repo: &TempRepo, names: &[&str], message: &str) -> String {
        for name in names {
            let blob = crate::test_support::git_command()
                .current_dir(repo.path())
                .args(["hash-object", "-w", "--stdin"])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .and_then(|mut child| {
                    use std::io::Write;
                    child.stdin.take().unwrap().write_all(name.as_bytes())?;
                    child.wait_with_output()
                })
                .unwrap();
            let info = format!(
                "100644,{},{name}",
                String::from_utf8(blob.stdout).unwrap().trim()
            );
            repo.git(&[
                "-c",
                "core.precomposeunicode=false",
                "update-index",
                "--add",
                "--cacheinfo",
                &info,
            ]);
        }
        repo.git(&["commit", "-q", "-m", message]);
        repo.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    // Rust says which files are binary by their type; the menus do not keep a list of their own.
    #[test]
    fn a_file_of_a_binary_type_says_so() {
        let repo = TempRepo::new();
        repo.commit("logo.PNG", b"not even an image", "png");
        repo.commit("notes.txt", b"text", "txt");
        fs::create_dir(repo.path().join("dir.png")).unwrap();

        let entries = list_dir(repo.path(), "").unwrap();
        let binary = |name: &str| {
            entries
                .iter()
                .find(|entry| entry.name == name)
                .unwrap()
                .binary
        };

        assert!(binary("logo.PNG"));
        assert!(!binary("notes.txt"));
        assert!(!binary("dir.png"));
    }

    fn entry(path: &str, dir: bool) -> TreeEntry {
        TreeEntry::new(
            path.rsplit('/').next().unwrap().to_string(),
            path.to_string(),
            dir,
        )
    }

    #[test]
    fn ls_tree_lists_one_level_of_a_revision() {
        let repo = TempRepo::new();
        let first = commit_exact(
            &repo,
            &["README.md", "src/a.txt", "src/lib/b.rs", "docs/x.md"],
            "one",
        );
        // A submodule is a `commit` entry: `GitDirectoryVirtualFile` makes it a file.
        let gitlink = format!("160000,{first},vendor");
        repo.git(&["update-index", "--add", "--cacheinfo", &gitlink]);
        repo.git(&["rm", "-q", "-r", "--cached", "docs"]);
        let second = commit_exact(&repo, &["new.txt"], "two");
        // What the disk has now is not what a revision has.
        touch(&repo, "src/on-disk.txt");

        assert_eq!(
            ls_tree(repo.path(), &first, "").unwrap(),
            [
                entry("docs", true),
                entry("src", true),
                entry("README.md", false)
            ]
        );
        let src = [entry("src/lib", true), entry("src/a.txt", false)];
        assert_eq!(ls_tree(repo.path(), &first, "src").unwrap(), src);
        assert_eq!(ls_tree(repo.path(), &first, "src/").unwrap(), src);
        assert_eq!(
            ls_tree(repo.path(), &first, "src/lib").unwrap(),
            [entry("src/lib/b.rs", false)]
        );
        assert_eq!(
            ls_tree(repo.path(), &second, "").unwrap(),
            [
                entry("src", true),
                entry("new.txt", false),
                entry("README.md", false),
                entry("vendor", false)
            ]
        );
        assert_eq!(ls_tree(repo.path(), &second, "src").unwrap(), src);
        // A folder the revision does not have, or a file, has no entries.
        assert_eq!(ls_tree(repo.path(), &second, "docs").unwrap(), []);
        assert_eq!(ls_tree(repo.path(), &second, "new.txt").unwrap(), []);
    }

    #[test]
    fn ls_tree_orders_as_the_file_chooser_and_hides_ignored_names_and_archives() {
        let repo = TempRepo::new();
        let head = commit_exact(
            &repo,
            &[
                "a10",
                "a2",
                "A1",
                "a-b",
                "a.b",
                ".DS_Store",
                "cache.pyc",
                "lib.jar",
                "Pack.ZIP",
                "notes~",
                "__pycache__/m.txt",
                "x.zip/inner.txt",
                ".env",
            ],
            "names",
        );

        // `StringUtil.naturalCompare`: `-` is not read as `_` there, as it is in the Project tree.
        assert_eq!(
            names(&ls_tree(repo.path(), &head, "").unwrap()),
            ["x.zip", ".env", "a-b", "a.b", "A1", "a2", "a10"]
        );
    }

    #[test]
    fn ls_tree_reaches_exactly_the_folder_it_names() {
        let repo = TempRepo::new();
        let nfc = "caf\u{e9}";
        let nfd = "cafe\u{301}";
        let (composed, decomposed) = (
            format!("{nfc}/composed.txt"),
            format!("{nfd}/decomposed.txt"),
        );
        let head = commit_exact(
            &repo,
            &[
                "[ab]/-x*?.txt",
                "b/a.txt",
                ":(glob)*/g.txt",
                "-d/x.txt",
                "with space/s.txt",
                composed.as_str(),
                decomposed.as_str(),
            ],
            "names",
        );
        // With it, git would look up the composed name for the decomposed one.
        repo.git(&["config", "core.precomposeunicode", "true"]);

        let paths = |dir: &str| -> Vec<String> {
            ls_tree(repo.path(), &head, dir)
                .unwrap()
                .into_iter()
                .map(|entry| entry.path)
                .collect()
        };
        assert_eq!(paths("[ab]"), ["[ab]/-x*?.txt"]);
        assert_eq!(paths(":(glob)*"), [":(glob)*/g.txt"]);
        assert_eq!(paths("-d"), ["-d/x.txt"]);
        assert_eq!(paths("with space"), ["with space/s.txt"]);
        assert_eq!(paths(nfc), [composed]);
        assert_eq!(paths(nfd), [decomposed]);
        // Not a pattern: `*` and `?` match nothing else.
        assert_eq!(paths("*"), Vec::<String>::new());
    }

    #[test]
    fn an_invalid_revision_or_folder_is_refused_before_git_runs() {
        // Not a repository: whatever git would say, the refusal comes first.
        let outside = TempDir::new().unwrap();
        let hash = "a".repeat(40);
        let with_path = format!("{hash}:x");
        for revision in [
            "HEAD",
            "-n",
            "--all",
            "abc",
            "a1b2c3d4",
            with_path.as_str(),
            "",
        ] {
            match ls_tree(outside.path(), revision, "") {
                Err(TreeError::Git(GitError::Io(message))) => {
                    assert!(message.starts_with("invalid hash"), "{revision}: {message}")
                }
                other => panic!("{revision}: {other:?}"),
            }
        }
        for dir in ["..", "src/../..", "/etc", "a//b", "./a", ".git", "a/.GIT/x"] {
            assert!(
                matches!(
                    ls_tree(outside.path(), &hash, dir),
                    Err(TreeError::Outside(_))
                ),
                "{dir}"
            );
        }
    }

    #[test]
    fn a_revision_that_does_not_exist_is_an_error() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");

        assert!(matches!(
            ls_tree(repo.path(), &"0".repeat(40), ""),
            Err(TreeError::Git(GitError::Failed { .. }))
        ));
    }
}
