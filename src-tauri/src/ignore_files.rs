// `Git.Ignore.File` of the Local Changes tab (`GitIgnoreFileActionGroup`): `Add to .gitignore` writes
// the unversioned files of the selection into a `.gitignore` that covers them all, into a new one at
// the root when none does (`CreateNewIgnoreFileAction`), or into `.git/info/exclude`
// (`AddToGitExcludeAction`). Each entry is the path from the folder of the file it goes into, after a
// slash, and a folder ends with a slash (`GitIgnoredFileContentProvider.buildIgnoreEntryContent`,
// `IgnoredBeanFactory.ignoreUnderDirectory`); the entries go at the end of the file, after a newline if
// it lacks one (`addNewElements`).
//
// Departs from IntelliJ (`docs/parity-matrix.md`): the characters git reads as a pattern are escaped,
// so that an entry names only its own file; only unversioned files and folders are taken; a missing
// exclude file is created instead of skipped; and a link is never written through. A line already in
// the file is written again, as IntelliJ does: the file is still unversioned, so a later rule (`!`)
// brings it back, and only a line after that one ignores it.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::git::{self, GitError};
use crate::snapshots;

const GITIGNORE: &str = ".gitignore";

// A file, or a folder of the tree (`ChangesListView.EXACTLY_SELECTED_FILES`).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Entry {
    pub path: String,
    pub directory: bool,
}

// What the group offers besides `.git/info/exclude`, which it always offers.
#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Targets {
    // The `.gitignore` files that cover every entry, relative to the root, the root's first.
    pub gitignores: Vec<String>,
    // None covers them and the root has none: `Add to .gitignore` creates it after asking.
    pub create: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Target {
    Gitignore { path: String },
    Create,
    Exclude,
}

fn io_error(error: io::Error) -> GitError {
    GitError::Io(error.to_string())
}

fn refused(message: String) -> GitError {
    GitError::Io(message)
}

fn parent(path: &str) -> &str {
    path.rfind('/').map_or("", |slash| &path[..slash])
}

// A regular file: a link could point anywhere, and a folder is not an ignore file.
fn is_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_file())
}

fn gitignore_in(folder: &str) -> String {
    if folder.is_empty() {
        GITIGNORE.to_string()
    } else {
        format!("{folder}/{GITIGNORE}")
    }
}

// The `.gitignore` files in the entry's folder and above it, up to the root (`findSuitableIgnoreFiles`:
// the ignore file's folder holds the file's folder).
fn suitable(repo: &Path, entry: &Entry) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut folder = parent(&entry.path);
    loop {
        let candidate = gitignore_in(folder);
        if is_file(&repo.join(&candidate)) {
            found.insert(candidate);
        }
        if folder.is_empty() {
            return found;
        }
        folder = parent(folder);
    }
}

fn check(repo: &Path, entries: &[Entry]) -> Result<(), GitError> {
    if entries.is_empty() {
        return Err(refused("No files to ignore".to_string()));
    }
    for entry in entries {
        if entry.path.contains(['\n', '\r']) {
            return Err(refused(format!(
                "\"{}\" cannot be written to an ignore file",
                entry.path.escape_debug()
            )));
        }
        snapshots::check_path(repo, &entry.path).map_err(io_error)?;
    }
    Ok(())
}

// Those that only one ignore file covers for all, in the root's first order.
pub fn targets(repo: &Path, entries: &[Entry]) -> Result<Targets, GitError> {
    check(repo, entries)?;
    let mut common: Option<BTreeSet<String>> = None;
    for entry in entries {
        let found = suitable(repo, entry);
        common = Some(match common {
            None => found,
            Some(common) => common.intersection(&found).cloned().collect(),
        });
    }
    let mut gitignores: Vec<String> = common.unwrap_or_default().into_iter().collect();
    gitignores.sort_by_key(|path| (path.matches('/').count(), path.clone()));
    let create = gitignores.is_empty() && fs::symlink_metadata(repo.join(GITIGNORE)).is_err();
    Ok(Targets { gitignores, create })
}

// Every entry must be unversioned: a file that git lists as such, or a folder that holds one. Git
// applies the user's ignore rules, so an ignored file is refused too.
fn check_unversioned(repo: &Path, entries: &[Entry]) -> Result<(), GitError> {
    let paths: Vec<String> = entries.iter().map(|entry| entry.path.clone()).collect();
    let listed = crate::unversioned::unversioned(repo, &paths)?;
    for entry in entries {
        let found = if entry.directory {
            let prefix = format!("{}/", entry.path);
            listed.iter().any(|path| path.starts_with(&prefix))
        } else {
            listed.contains(&entry.path)
        };
        if !found {
            return Err(refused(format!("\"{}\" is not unversioned", entry.path)));
        }
    }
    Ok(())
}

// Git reads `\`, `*`, `?` and `[` as a pattern and drops spaces at the end: with a backslash each one
// stands for itself. The entry starts with a slash, so `#` and `!` need nothing.
fn escaped(path: &str) -> String {
    let mut text = String::with_capacity(path.len());
    for character in path.chars() {
        if matches!(character, '\\' | '*' | '?' | '[') {
            text.push('\\');
        }
        text.push(character);
    }
    let kept = text.trim_end_matches(' ').len();
    let spaces = text.len() - kept;
    text.truncate(kept);
    text.push_str(&"\\ ".repeat(spaces));
    text
}

// The line of `entry` in an ignore file whose folder is `base` ("" for the root).
fn line(base: &str, entry: &Entry) -> String {
    let relative = if base.is_empty() {
        entry.path.as_str()
    } else {
        &entry.path[base.len() + 1..]
    };
    let slash = if entry.directory { "/" } else { "" };
    format!("/{}{slash}", escaped(relative))
}

// A folder's files are left out: its own line already covers them.
fn without_covered(entries: &[Entry]) -> Vec<Entry> {
    let unique: BTreeSet<Entry> = entries.iter().cloned().collect();
    let folders: Vec<String> = unique
        .iter()
        .filter(|entry| entry.directory)
        .map(|entry| format!("{}/", entry.path))
        .collect();
    unique
        .into_iter()
        .filter(|entry| !folders.iter().any(|folder| entry.path.starts_with(folder)))
        .collect()
}

fn exclude_file(repo: &Path) -> Result<PathBuf, GitError> {
    let output = git::run(repo, &["rev-parse", "--git-path", "info/exclude"])?;
    let text = String::from_utf8_lossy(&output).trim_end_matches('\n').to_string();
    Ok(repo.join(text))
}

fn append(file: &Path, create: bool, lines: &[String]) -> Result<(), GitError> {
    let existing = match fs::symlink_metadata(file) {
        Ok(meta) if meta.file_type().is_file() => fs::read(file).map_err(io_error)?,
        Ok(_) => {
            return Err(refused(format!(
                "{} is not a regular file",
                file.display()
            )))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound && create => Vec::new(),
        Err(error) => return Err(io_error(error)),
    };
    let mut seen = BTreeSet::new();
    let new: Vec<&String> = lines.iter().filter(|line| seen.insert(line.as_str())).collect();
    let mut written = String::new();
    if existing.last().is_some_and(|last| *last != b'\n') {
        written.push('\n');
    }
    for line in new {
        written.push_str(line);
        written.push('\n');
    }
    if let Some(folder) = file.parent() {
        fs::create_dir_all(folder).map_err(io_error)?;
    }
    let mut options = OpenOptions::new();
    options.append(true).create(create);
    options
        .open(file)
        .and_then(|mut handle| handle.write_all(written.as_bytes()))
        .map_err(io_error)
}

// Writes the entries into `target`. Nothing is written unless every entry is unversioned and the
// target is one the group offers now.
pub fn add(repo: &Path, target: &Target, entries: &[Entry]) -> Result<(), GitError> {
    let offered = targets(repo, entries)?;
    check_unversioned(repo, entries)?;
    let entries = without_covered(entries);
    let (file, base, create) = match target {
        Target::Gitignore { path } => {
            if !offered.gitignores.contains(path) {
                return Err(refused(format!("{path} does not cover every file")));
            }
            (repo.join(path), parent(path).to_string(), false)
        }
        Target::Create => {
            if !offered.create {
                return Err(refused(format!("{GITIGNORE} already exists")));
            }
            (repo.join(GITIGNORE), String::new(), true)
        }
        Target::Exclude => (exclude_file(repo)?, String::new(), true),
    };
    let lines: Vec<String> = entries.iter().map(|entry| line(&base, entry)).collect();
    append(&file, create, &lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempRepo;

    fn write(repo: &Path, path: &str, text: &str) {
        let target = repo.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, text).unwrap();
    }

    fn file(path: &str) -> Entry {
        Entry {
            path: path.to_string(),
            directory: false,
        }
    }

    fn folder(path: &str) -> Entry {
        Entry {
            path: path.to_string(),
            directory: true,
        }
    }

    fn status(repo: &TempRepo) -> String {
        repo.git(&["status", "--porcelain", "--untracked-files=all"])
    }

    fn exclude(repo: &TempRepo) -> String {
        fs::read_to_string(repo.path().join(".git/info/exclude")).unwrap()
    }

    #[test]
    fn files_and_a_folder_go_to_the_exclude_file_from_the_root_and_leave_the_status() {
        let repo = TempRepo::new();
        repo.commit("tracked.txt", b"one\n", "first");
        write(repo.path(), "notes.md", "new\n");
        write(repo.path(), "video/.hyperframes/a.jpg", "new\n");
        write(repo.path(), "video/.hyperframes/b.jpg", "new\n");
        let before = exclude(&repo);

        add(
            repo.path(),
            &Target::Exclude,
            &[file("notes.md"), folder("video/.hyperframes"), file("video/.hyperframes/a.jpg")],
        )
        .unwrap();

        let text = exclude(&repo);
        assert_eq!(&text[before.len()..], "/notes.md\n/video/.hyperframes/\n");
        assert_eq!(status(&repo), "");
    }

    #[test]
    fn a_gitignore_gets_the_path_from_its_own_folder_after_a_newline_it_lacked() {
        let repo = TempRepo::new();
        write(repo.path(), "web/.gitignore", "dist");
        repo.git(&["add", "web/.gitignore"]);
        repo.git(&["commit", "-q", "-m", "ignore"]);
        write(repo.path(), "web/src/draft.ts", "new\n");

        add(
            repo.path(),
            &Target::Gitignore {
                path: "web/.gitignore".into(),
            },
            &[file("web/src/draft.ts")],
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(repo.path().join("web/.gitignore")).unwrap(),
            "dist\n/src/draft.ts\n"
        );
        assert_eq!(status(&repo), " M web/.gitignore\n");
    }

    #[test]
    fn the_targets_are_the_gitignores_that_cover_every_file_root_first() {
        let repo = TempRepo::new();
        repo.commit("tracked.txt", b"one\n", "first");
        write(repo.path(), ".gitignore", "x\n");
        write(repo.path(), "web/.gitignore", "x\n");
        write(repo.path(), "web/src/.gitignore", "x\n");
        write(repo.path(), "web/src/a.ts", "new\n");
        write(repo.path(), "web/b.ts", "new\n");

        let one = targets(repo.path(), &[file("web/src/a.ts")]).unwrap();
        let both = targets(repo.path(), &[file("web/src/a.ts"), file("web/b.ts")]).unwrap();
        // A folder's own `.gitignore` is inside it, not above it.
        let src = targets(repo.path(), &[folder("web/src")]).unwrap();

        assert_eq!(one.gitignores, [".gitignore", "web/.gitignore", "web/src/.gitignore"]);
        assert_eq!(both.gitignores, [".gitignore", "web/.gitignore"]);
        assert_eq!(src.gitignores, [".gitignore", "web/.gitignore"]);
        assert!(!one.create);
    }

    #[test]
    fn without_any_gitignore_one_is_offered_at_the_root_and_created() {
        let repo = TempRepo::new();
        repo.commit("tracked.txt", b"one\n", "first");
        write(repo.path(), "docs/new.md", "new\n");
        let entries = [file("docs/new.md")];

        let offered = targets(repo.path(), &entries).unwrap();
        add(repo.path(), &Target::Create, &entries).unwrap();

        assert_eq!(offered, Targets { gitignores: vec![], create: true });
        assert_eq!(fs::read_to_string(repo.path().join(".gitignore")).unwrap(), "/docs/new.md\n");
        assert!(add(repo.path(), &Target::Create, &entries).is_err());
    }

    #[test]
    fn names_with_pattern_characters_and_trailing_spaces_match_only_themselves() {
        let repo = TempRepo::new();
        repo.commit("tracked.txt", b"one\n", "first");
        for name in ["my [draft].md", "my d.md", "a*b.txt", "axxb.txt", "end ", "#hash", "!bang"] {
            write(repo.path(), name, "new\n");
        }

        add(
            repo.path(),
            &Target::Exclude,
            &[file("my [draft].md"), file("a*b.txt"), file("end "), file("#hash"), file("!bang")],
        )
        .unwrap();

        assert_eq!(status(&repo), "?? axxb.txt\n?? \"my d.md\"\n");
    }

    #[test]
    fn a_line_that_a_later_rule_undoes_is_written_again_and_the_file_goes() {
        let repo = TempRepo::new();
        repo.commit("tracked.txt", b"one\n", "first");
        write(repo.path(), "notes.md", "new\n");
        fs::write(repo.path().join(".git/info/exclude"), "/notes.md\r\n!/notes.md").unwrap();

        add(repo.path(), &Target::Exclude, &[file("notes.md"), file("notes.md")]).unwrap();

        assert_eq!(exclude(&repo), "/notes.md\r\n!/notes.md\n/notes.md\n");
        assert_eq!(status(&repo), "");
    }

    #[test]
    fn a_linked_worktree_writes_the_exclude_file_of_the_common_folder() {
        let repo = TempRepo::new();
        repo.commit("tracked.txt", b"one\n", "first");
        let linked = repo.worktree("side");
        write(linked.path(), "local.env", "new\n");
        let before = exclude(&repo);

        add(linked.path(), &Target::Exclude, &[file("local.env")]).unwrap();

        assert_eq!(&exclude(&repo)[before.len()..], "/local.env\n");
        assert_eq!(status(&linked), "");
    }

    #[test]
    fn a_missing_exclude_file_is_created() {
        let repo = TempRepo::new();
        repo.commit("tracked.txt", b"one\n", "first");
        fs::remove_dir_all(repo.path().join(".git/info")).unwrap();
        write(repo.path(), "notes.md", "new\n");

        add(repo.path(), &Target::Exclude, &[file("notes.md")]).unwrap();

        assert_eq!(exclude(&repo), "/notes.md\n");
    }

    #[test]
    fn anything_but_unversioned_files_or_an_offered_target_writes_nothing() {
        let repo = TempRepo::new();
        repo.commit("tracked.txt", b"one\n", "first");
        write(repo.path(), "web/.gitignore", "ignored.log\n");
        write(repo.path(), "new.md", "new\n");
        write(repo.path(), "web/ignored.log", "new\n");
        write(repo.path(), "empty/keep.txt", "");
        repo.git(&["add", "empty/keep.txt"]);
        let before = exclude(&repo);
        for bad in [
            file("tracked.txt"),
            file("web/ignored.log"),
            file("missing.md"),
            file("../outside.md"),
            file(".git/config"),
            file("new\nline.md"),
            folder("empty"),
        ] {
            assert!(
                add(repo.path(), &Target::Exclude, &[file("new.md"), bad.clone()]).is_err(),
                "{bad:?} was accepted"
            );
        }
        // `web/.gitignore` does not cover a file at the root.
        let wrong = Target::Gitignore {
            path: "web/.gitignore".into(),
        };
        assert!(add(repo.path(), &wrong, &[file("new.md")]).is_err());
        assert!(add(repo.path(), &Target::Exclude, &[]).is_err());
        assert_eq!(exclude(&repo), before);
        assert_eq!(fs::read_to_string(repo.path().join("web/.gitignore")).unwrap(), "ignored.log\n");
    }

    #[cfg(unix)]
    #[test]
    fn a_gitignore_that_is_a_link_is_not_offered_nor_written_through() {
        let repo = TempRepo::new();
        repo.commit("tracked.txt", b"one\n", "first");
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::os::unix::fs::symlink(outside.path(), repo.path().join(".gitignore")).unwrap();
        write(repo.path(), "new.md", "new\n");
        let entries = [file("new.md")];

        let offered = targets(repo.path(), &entries).unwrap();

        assert_eq!(offered, Targets { gitignores: vec![], create: false });
        let through = Target::Gitignore {
            path: ".gitignore".into(),
        };
        assert!(add(repo.path(), &through, &entries).is_err());
        assert!(add(repo.path(), &Target::Create, &entries).is_err());
        assert_eq!(fs::read_to_string(outside.path()).unwrap(), "");
    }
}
