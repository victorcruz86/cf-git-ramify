// What Copy as Patch and Create Patch read and write (`CreatePatchCommitExecutor`,
// `CreatePatchFromChangesAction`, `PatchWriter.writePatches` and `writeAsPatchToClipboard`).
//
// Each change comes as two versions of a file: HEAD, the index, the disk (in the form git would
// store it), a commit, or HEAD with the chosen lines of the disk (a changelist's part of a file).
// The contents are read here, in Rust: they never travel through JSON.
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tempfile::TempDir;

use super::{build, write, PatchError, PatchSource, WriteOptions};
use crate::diff;
use crate::git::{self, GitError};
use crate::hunks::{self, LineSelection};
use crate::snapshots;
use crate::storage;

const GITLINK_MODE: &str = "160000";
const BLOB_MODES: [&str; 3] = ["100644", "100755", "120000"];

// One version of a file. `Disk` is what `git add` would store of the file now; `Selection` is
// HEAD with the chosen changes of the disk applied (`hunks::partial_file`), and only goes as the
// after side.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Version {
    Head,
    Index,
    Disk,
    Revision { hash: String },
    Selection { selection: Vec<LineSelection> },
}

// A change: `before: None` is a new file and `after: None` a deleted one. `old_path` is the name
// of the before side of a rename; HEAD and commits are read at it, the index and the disk at
// `path` (the index already has the new name).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchEntry {
    pub path: String,
    pub old_path: Option<String>,
    pub before: Option<Version>,
    pub after: Option<Version>,
}

// `Base path:`, `Reverse patch` and the message of the `Subject:` header. An empty base path is
// the repository.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateOptions {
    pub base_path: String,
    pub reverse: bool,
    pub message: Option<String>,
}

// `To file:` or `To clipboard`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PatchTarget {
    File { path: String },
    Clipboard,
}

// What a version has at a path: nothing, a file's bytes, or a submodule (which gives no patch).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Content {
    Absent,
    Bytes(Vec<u8>),
    Gitlink,
}

fn failure(text: impl Into<String>) -> PatchError {
    PatchError::Git(GitError::Io(text.into()))
}

// One record of `ls-tree -z` or `ls-files -s -z` for exactly `path`: mode, hash and the third
// field (type or stage). A folder lists its own entry (ls-tree) or what is inside (ls-files),
// never a record with its exact name and a blob mode.
fn record(output: &[u8], path: &str) -> Option<[String; 3]> {
    output.split(|byte| *byte == 0).find_map(|line| {
        let tab = line.iter().position(|byte| *byte == b'\t')?;
        if &line[tab + 1..] != path.as_bytes() {
            return None;
        }
        let head = String::from_utf8_lossy(&line[..tab]).into_owned();
        let mut fields = head.split_whitespace().map(str::to_string);
        Some([fields.next()?, fields.next()?, fields.next()?])
    })
}

fn blob(repo: &Path, oid: &str) -> Result<Vec<u8>, PatchError> {
    diff::check_hash(oid)?;
    Ok(git::run(repo, &["cat-file", "blob", oid])?)
}

fn by_mode(repo: &Path, mode: &str, oid: &str) -> Result<Content, PatchError> {
    if mode == GITLINK_MODE {
        return Ok(Content::Gitlink);
    }
    if !BLOB_MODES.contains(&mode) {
        return Ok(Content::Absent);
    }
    Ok(Content::Bytes(blob(repo, oid)?))
}

// The versions of one call share HEAD's hash and the copy of the index the disk is stored in.
// `stored` holds what `git add` stored there, by path; `adds` counts the `git add` runs.
struct Reader<'a> {
    repo: &'a Path,
    head: Option<String>,
    index: Option<TempDir>,
    stored: HashMap<String, [String; 3]>,
    adds: usize,
}

impl<'a> Reader<'a> {
    fn new(repo: &'a Path) -> Self {
        Self {
            repo,
            head: None,
            index: None,
            stored: HashMap::new(),
            adds: 0,
        }
    }

    // Empty without commits.
    fn head(&mut self) -> Result<String, PatchError> {
        if self.head.is_none() {
            let head = match git::run(self.repo, &["rev-parse", "-q", "--verify", "HEAD"]) {
                Ok(output) => String::from_utf8_lossy(&output).trim().to_string(),
                Err(GitError::Failed { code: Some(1), .. }) => String::new(),
                Err(error) => return Err(error.into()),
            };
            self.head = Some(head);
        }
        Ok(self.head.clone().unwrap_or_default())
    }

    fn tree(&self, commit: &str, path: &str) -> Result<Content, PatchError> {
        let args = git::exact(&["--literal-pathspecs", "ls-tree", "-z", commit, "--", path]);
        match record(&git::run(self.repo, &args)?, path) {
            Some([mode, _, oid]) => by_mode(self.repo, &mode, &oid),
            None => Ok(Content::Absent),
        }
    }

    fn index(&self, path: &str) -> Result<Content, PatchError> {
        let args = git::exact(&["--literal-pathspecs", "ls-files", "-s", "-z", "--", path]);
        let output = git::run(self.repo, &args)?;
        match record(&output, path) {
            Some([_, _, stage]) if stage != "0" => Err(failure(format!(
                "\"{path}\" has unresolved conflicts: resolve them before continuing"
            ))),
            Some([mode, oid, _]) => by_mode(self.repo, &mode, &oid),
            None => Ok(Content::Absent),
        }
    }

    // What `git add` would store: on a copy of the index, made once per call and deleted at the
    // end. The real index is not touched. The paths of a call go in together: one `git add` and one
    // `ls-files` (in batches of `PATHS_PER_COMMAND`, for the argument limit).
    fn store(&mut self, paths: &[String]) -> Result<(), PatchError> {
        let paths: Vec<String> = paths
            .iter()
            .filter(|path| !self.stored.contains_key(path.as_str()))
            .filter(|path| matches!(on_disk(self.repo, path), Ok(true)))
            .cloned()
            .collect();
        if paths.is_empty() {
            return Ok(());
        }
        if self.index.is_none() {
            let temp = TempDir::new().map_err(|error| failure(error.to_string()))?;
            match fs::copy(
                git::git_path(self.repo, "index")?,
                temp.path().join("index"),
            ) {
                Ok(_) => {}
                // A repository without an index yet: git starts an empty one.
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(failure(error.to_string())),
            }
            self.index = Some(temp);
        }
        let index = self.index.as_ref().expect("just made").path().join("index");
        let envs = [("GIT_INDEX_FILE", index.as_path())];
        for chunk in paths.chunks(git::PATHS_PER_COMMAND) {
            let mut add = git::exact(&["--literal-pathspecs", "add", "--force", "--"]);
            add.extend(chunk.iter().map(String::as_str));
            git::run_with_env(self.repo, &add, &envs)?;
            self.adds += 1;
            let mut list = git::exact(&["--literal-pathspecs", "ls-files", "-s", "-z", "--"]);
            list.extend(chunk.iter().map(String::as_str));
            let output = git::run_with_env(self.repo, &list, &envs)?;
            for path in chunk {
                if let Some(entry) = record(&output, path) {
                    self.stored.insert(path.clone(), entry);
                }
            }
        }
        Ok(())
    }

    fn disk(&mut self, path: &str) -> Result<Content, PatchError> {
        if !on_disk(self.repo, path)? {
            return Ok(Content::Absent);
        }
        self.store(&[path.to_string()])?;
        match self.stored.get(path) {
            Some([mode, oid, _]) => by_mode(self.repo, mode, oid),
            None => Err(failure(format!("git did not store \"{path}\""))),
        }
    }

    fn revision(&self, hash: &str, path: &str) -> Result<Content, PatchError> {
        diff::check_hash(hash)?;
        let commit = format!("{hash}^{{commit}}");
        git::run(self.repo, &["rev-parse", "-q", "--verify", &commit])
            .map_err(|error| not_a_commit(hash, error))?;
        self.tree(hash, path)
    }

    fn read(&mut self, version: &Version, path: &str) -> Result<Content, PatchError> {
        match version {
            Version::Head => {
                let head = self.head()?;
                if head.is_empty() {
                    Ok(Content::Absent)
                } else {
                    self.tree(&head, path)
                }
            }
            Version::Index => self.index(path),
            Version::Disk => self.disk(path),
            Version::Revision { hash } => self.revision(hash, path),
            Version::Selection { selection } => hunks::partial_file(self.repo, path, selection)
                .map(|file| Content::Bytes(file.content))
                .map_err(|error| failure(error.to_string())),
        }
    }

    // `(revision <hash>)` of the before side: the commit, or HEAD for the index and HEAD itself.
    fn revision_of(&mut self, version: &Version) -> Result<Option<String>, PatchError> {
        Ok(match version {
            Version::Revision { hash } => Some(hash.clone()),
            _ => Some(self.head()?).filter(|head| !head.is_empty()),
        })
    }
}

// A file (or link) at `path`; a folder, or nothing, is no file.
fn on_disk(repo: &Path, path: &str) -> Result<bool, PatchError> {
    match fs::symlink_metadata(repo.join(path)) {
        Ok(meta) => Ok(!meta.is_dir()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(false)
        }
        Err(error) => Err(failure(error.to_string())),
    }
}

// `rev-parse -q --verify <x>^{commit}` exits with 1 for an unknown object and for one that is not a
// commit. Anything else (git missing, a damaged repository) is passed on as it is.
fn not_a_commit(hash: &str, error: GitError) -> PatchError {
    match error {
        GitError::Failed { code: Some(1), .. } => failure(format!("{hash} is not a commit")),
        error => PatchError::Git(error),
    }
}

fn check(repo: &Path, path: &str) -> Result<(), PatchError> {
    snapshots::check_path(repo, path).map_err(|error| failure(error.to_string()))
}

// One change as the builder takes it, or `None` when there is nothing to write: a submodule on
// either side, or two absent sides.
fn source(reader: &mut Reader, entry: &PatchEntry) -> Result<Option<PatchSource>, PatchError> {
    let path = entry.path.as_str();
    check(reader.repo, path)?;
    if let Some(old) = &entry.old_path {
        check(reader.repo, old)?;
    }
    if matches!(
        entry.before,
        Some(Version::Disk | Version::Selection { .. })
    ) {
        return Err(failure(format!(
            "\"{path}\": the disk is never the before side"
        )));
    }
    let before_path = match &entry.before {
        Some(Version::Index) | None => path,
        Some(_) => entry.old_path.as_deref().unwrap_or(path),
    };
    let before = match &entry.before {
        Some(version) => reader.read(version, before_path)?,
        None => Content::Absent,
    };
    let after = match &entry.after {
        Some(version) => reader.read(version, path)?,
        None => Content::Absent,
    };
    let (before, after) = match (before, after) {
        (Content::Gitlink, _) | (_, Content::Gitlink) => return Ok(None),
        (Content::Absent, Content::Absent) => return Ok(None),
        (before, after) => (before, after),
    };
    let bytes = |content: Content| match content {
        Content::Bytes(bytes) => Some(bytes),
        _ => None,
    };
    let (before, after) = (bytes(before), bytes(after));
    let binary = [&before, &after]
        .iter()
        .any(|side| side.as_deref().is_some_and(diff::looks_binary));
    let before_rev = match (&entry.before, &before) {
        (Some(version), Some(_)) => reader.revision_of(version)?,
        _ => None,
    };
    Ok(Some(PatchSource {
        before: before.map(|content| (before_path.to_string(), content)),
        after: after.map(|content| (path.to_string(), content)),
        before_rev,
        binary,
    }))
}

// A file of a changelist: HEAD against what the list has of it, which is the whole file as git
// would store it, or only its hunks (`selection`) when the file is split across lists or lines
// were chosen.
pub fn changelist_entry(
    path: &str,
    old_path: Option<&str>,
    selection: Option<&[LineSelection]>,
) -> PatchEntry {
    let after = match selection {
        Some(selection) => Version::Selection {
            selection: selection.to_vec(),
        },
        None => Version::Disk,
    };
    PatchEntry {
        path: path.to_string(),
        old_path: old_path.map(str::to_string),
        before: Some(Version::Head),
        after: Some(after),
    }
}

// The files of Shelve and of the Update saver, read with one `Reader`: the disk versions of every
// valid path are stored together first. Each file gets what reading it alone gives, its error
// included: if storing them together fails, each one is stored again on its own.
pub fn changelist_contents(
    repo: &Path,
    entries: &[PatchEntry],
) -> Vec<Result<Option<PatchSource>, PatchError>> {
    changelist_contents_with(&mut Reader::new(repo), entries)
}

fn changelist_contents_with(
    reader: &mut Reader,
    entries: &[PatchEntry],
) -> Vec<Result<Option<PatchSource>, PatchError>> {
    let valid = |entry: &PatchEntry| {
        check(reader.repo, &entry.path).is_ok()
            && entry
                .old_path
                .as_deref()
                .is_none_or(|old| check(reader.repo, old).is_ok())
    };
    let disk: Vec<String> = entries
        .iter()
        .filter(|entry| entry.after == Some(Version::Disk) && valid(entry))
        .map(|entry| entry.path.clone())
        .collect();
    let _ = reader.store(&disk);
    entries.iter().map(|entry| source(reader, entry)).collect()
}

// Every path is checked first, then the disk versions are stored together.
fn read_all(reader: &mut Reader, entries: &[PatchEntry]) -> Result<Vec<PatchSource>, PatchError> {
    let mut disk = Vec::new();
    for entry in entries {
        check(reader.repo, &entry.path)?;
        if let Some(old) = &entry.old_path {
            check(reader.repo, old)?;
        }
        if entry.after == Some(Version::Disk) {
            disk.push(entry.path.clone());
        }
    }
    reader.store(&disk)?;
    let mut sources = Vec::new();
    for entry in entries {
        sources.extend(source(reader, entry)?);
    }
    Ok(sources)
}

pub fn sources(repo: &Path, entries: &[PatchEntry]) -> Result<Vec<PatchSource>, PatchError> {
    read_all(&mut Reader::new(repo), entries)
}

// The patch as it is written, with `\n` and in UTF-8 (decision 6).
pub fn text(
    repo: &Path,
    entries: &[PatchEntry],
    options: &CreateOptions,
) -> Result<Vec<u8>, PatchError> {
    text_of_sources(repo, &sources(repo, entries)?, options)
}

// The same, for changes already read (the shelf's).
pub fn text_of_sources(
    repo: &Path,
    sources: &[PatchSource],
    options: &CreateOptions,
) -> Result<Vec<u8>, PatchError> {
    let base_path = if options.base_path.trim().is_empty() {
        repo.to_path_buf()
    } else {
        PathBuf::from(&options.base_path)
    };
    if !base_path.is_absolute() {
        return Err(failure(format!(
            "The base path \"{}\" is not absolute",
            options.base_path
        )));
    }
    let write_options = WriteOptions {
        include_base: false,
        reverse: options.reverse,
        base_path,
    };
    let mut patch = build(repo, sources, &write_options)?;
    patch.header.message = options.message.clone();
    Ok(write(&patch, &write_options))
}

// `Files.createDirectories(file.getParent())` and the write, through a temporary file and a rename:
// a half-written patch never takes the place of the one there was.
pub fn write_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Cannot write patch to specified file: {}", path.display()),
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Cannot write patch to specified file: {}", path.display()),
        )
    })?;
    fs::create_dir_all(parent)?;
    if path.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Cannot write patch to specified file: {}", path.display()),
        ));
    }
    storage::write_atomic(path, bytes)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::patch::{read, FilePatch, LineKind, TextFilePatch};
    use crate::test_support::TempRepo;

    fn local(path: &str) -> PatchEntry {
        PatchEntry {
            path: path.to_string(),
            old_path: None,
            before: Some(Version::Head),
            after: Some(Version::Disk),
        }
    }

    fn texts(patch: &[u8]) -> Vec<TextFilePatch> {
        read(patch)
            .unwrap()
            .files
            .into_iter()
            .map(|file| match file {
                FilePatch::Text(text) => text,
                FilePatch::Binary(_) => panic!("a binary file"),
            })
            .collect()
    }

    fn added(file: &TextFilePatch) -> Vec<String> {
        file.hunks
            .iter()
            .flat_map(|hunk| &hunk.lines)
            .filter(|line| line.kind == LineKind::Added)
            .map(|line| String::from_utf8_lossy(&line.text).into_owned())
            .collect()
    }

    #[test]
    fn local_changes_are_head_against_what_git_would_store() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\n", "a");
        repo.commit("gone.txt", b"bye\n", "gone");
        let head = repo.commit("old.txt", b"same\nsame\nsame\n", "old");
        fs::write(repo.path().join("a.txt"), b"one\ntwo\n").unwrap();
        fs::remove_file(repo.path().join("gone.txt")).unwrap();
        fs::write(repo.path().join("new.txt"), b"fresh\n").unwrap();
        repo.git(&["mv", "old.txt", "renamed.txt"]);
        fs::write(repo.path().join("renamed.txt"), b"same\nsame\nsame\nmore\n").unwrap();

        let entries = [
            local("a.txt"),
            local("gone.txt"),
            PatchEntry {
                before: None,
                ..local("new.txt")
            },
            PatchEntry {
                old_path: Some("old.txt".to_string()),
                ..local("renamed.txt")
            },
        ];
        let sources = sources(repo.path(), &entries).unwrap();

        assert_eq!(
            sources[0].before,
            Some(("a.txt".to_string(), b"one\n".to_vec()))
        );
        assert_eq!(
            sources[0].after,
            Some(("a.txt".to_string(), b"one\ntwo\n".to_vec()))
        );
        assert_eq!(sources[0].before_rev.as_deref(), Some(head.as_str()));
        assert_eq!(sources[1].after, None);
        assert_eq!(sources[2].before, None);
        assert_eq!(sources[2].before_rev, None);
        assert_eq!(sources[3].before.as_ref().unwrap().0, "old.txt");
        assert_eq!(sources[3].after.as_ref().unwrap().0, "renamed.txt");
        // The real index is left as it was.
        assert_eq!(
            repo.git(&["status", "--porcelain", "--", "new.txt"]),
            "?? new.txt\n"
        );
    }

    #[test]
    fn a_partial_changelist_writes_only_its_hunks() {
        let repo = TempRepo::new();
        let lines: Vec<String> = (1..=20).map(|n| format!("{n}\n")).collect();
        repo.commit("a.txt", lines.concat().as_bytes(), "a");
        let mut changed = lines.clone();
        changed[1] = "two\n".to_string();
        changed[17] = "eighteen\n".to_string();
        fs::write(repo.path().join("a.txt"), changed.concat()).unwrap();
        let hunks = hunks::file_hunks(repo.path(), "a.txt").unwrap();
        assert_eq!(hunks.len(), 2);
        let selection = [LineSelection {
            hunk: hunks[1].id.clone(),
            lines: None,
        }];

        let entry = changelist_entry("a.txt", None, Some(&selection));
        let source = changelist_contents(repo.path(), &[entry])
            .remove(0)
            .unwrap()
            .unwrap();
        let patch = text(
            repo.path(),
            &[PatchEntry {
                after: Some(Version::Selection {
                    selection: selection.to_vec(),
                }),
                ..local("a.txt")
            }],
            &CreateOptions::default(),
        )
        .unwrap();

        let mut expected = lines.clone();
        expected[17] = "eighteen\n".to_string();
        assert_eq!(source.after.unwrap().1, expected.concat().into_bytes());
        let files = texts(&patch);
        assert_eq!(files[0].hunks.len(), 1);
        assert_eq!(added(&files[0]), ["eighteen"]);
    }

    // `GitStageCreatePatchActionProvider`: HEAD→index, index→disk and HEAD→disk.
    #[test]
    fn staging_versions_read_head_the_index_and_the_disk() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"head\n", "a");
        fs::write(repo.path().join("a.txt"), b"staged\n").unwrap();
        repo.git(&["add", "a.txt"]);
        fs::write(repo.path().join("a.txt"), b"disk\n").unwrap();
        fs::write(repo.path().join("u.txt"), b"untracked\n").unwrap();
        let entry = |before, after| PatchEntry {
            path: "a.txt".to_string(),
            old_path: None,
            before: Some(before),
            after: Some(after),
        };

        let read = sources(
            repo.path(),
            &[
                entry(Version::Head, Version::Index),
                entry(Version::Index, Version::Disk),
                entry(Version::Head, Version::Disk),
                PatchEntry {
                    path: "u.txt".to_string(),
                    old_path: None,
                    before: Some(Version::Index),
                    after: Some(Version::Disk),
                },
            ],
        )
        .unwrap();

        type Sides = (Option<Vec<u8>>, Option<Vec<u8>>);
        let sides: Vec<Sides> = read
            .into_iter()
            .map(|source| {
                (
                    source.before.map(|side| side.1),
                    source.after.map(|side| side.1),
                )
            })
            .collect();
        assert_eq!(
            sides,
            [
                (Some(b"head\n".to_vec()), Some(b"staged\n".to_vec())),
                (Some(b"staged\n".to_vec()), Some(b"disk\n".to_vec())),
                (Some(b"head\n".to_vec()), Some(b"disk\n".to_vec())),
                (None, Some(b"untracked\n".to_vec())),
            ]
        );
    }

    // The log's changes tree: a commit against its parent, with the hash checked by git.
    #[test]
    fn revisions_are_checked_and_read_at_their_paths() {
        let repo = TempRepo::new();
        let first = repo.commit("a.txt", b"one\n", "a");
        repo.git(&["mv", "a.txt", "b.txt"]);
        let second = repo.commit("b.txt", b"one\ntwo\n", "b");
        let entry = |before: &str| PatchEntry {
            path: "b.txt".to_string(),
            old_path: Some("a.txt".to_string()),
            before: Some(Version::Revision {
                hash: before.to_string(),
            }),
            after: Some(Version::Revision {
                hash: second.clone(),
            }),
        };

        let read = sources(repo.path(), &[entry(&first)]).unwrap();
        let tree = repo.git(&["rev-parse", "HEAD^{tree}"]).trim().to_string();

        assert_eq!(
            read[0].before,
            Some(("a.txt".to_string(), b"one\n".to_vec()))
        );
        assert_eq!(
            read[0].after,
            Some(("b.txt".to_string(), b"one\ntwo\n".to_vec()))
        );
        assert_eq!(read[0].before_rev.as_deref(), Some(first.as_str()));
        for wrong in ["--output=/tmp/x", "HEAD", tree.as_str()] {
            assert!(sources(repo.path(), &[entry(wrong)]).is_err(), "{wrong}");
        }
    }

    // One `git add` and one `ls-files` on the copy of the index for every disk version of a call,
    // not two per file.
    #[test]
    fn the_disk_versions_are_stored_with_one_git_add() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\n", "a");
        repo.commit("b.txt", b"one\n", "b");
        fs::write(repo.path().join("a.txt"), b"two\n").unwrap();
        fs::write(repo.path().join("b.txt"), b"two\n").unwrap();
        fs::write(repo.path().join("c.txt"), b"new\n").unwrap();
        fs::remove_file(repo.path().join("b.txt")).unwrap();
        let entries = [
            local("a.txt"),
            local("b.txt"),
            PatchEntry {
                before: None,
                ..local("c.txt")
            },
        ];
        let mut reader = Reader::new(repo.path());

        let read = read_all(&mut reader, &entries).unwrap();

        assert_eq!(reader.adds, 1);
        assert_eq!(read[0].after.as_ref().unwrap().1, b"two\n");
        assert_eq!(read[1].after, None);
        assert_eq!(read[2].after.as_ref().unwrap().1, b"new\n");
    }

    // Shelve and the Update saver read every file of their lists with one `Reader`: one `git add`
    // on one copy of the index for all of them, and for each file what reading it alone gives,
    // its error included.
    #[test]
    fn the_files_of_a_shelve_are_stored_with_one_git_add() {
        let repo = TempRepo::new();
        let lines: Vec<String> = (1..=20).map(|n| format!("{n}\n")).collect();
        repo.commit("a.txt", b"one\n", "a");
        repo.commit("b.txt", b"one\n", "b");
        repo.commit("d.txt", b"gone\n", "d");
        repo.commit("p.txt", lines.concat().as_bytes(), "p");
        fs::write(repo.path().join("a.txt"), b"two\n").unwrap();
        fs::write(repo.path().join("b.txt"), b"three\n").unwrap();
        fs::write(repo.path().join("c.txt"), b"new\n").unwrap();
        fs::remove_file(repo.path().join("d.txt")).unwrap();
        let mut changed = lines.clone();
        changed[1] = "two\n".to_string();
        changed[17] = "eighteen\n".to_string();
        fs::write(repo.path().join("p.txt"), changed.concat()).unwrap();
        let hunks = hunks::file_hunks(repo.path(), "p.txt").unwrap();
        let selection = [LineSelection {
            hunk: hunks[1].id.clone(),
            lines: None,
        }];
        let entries = [
            changelist_entry("a.txt", None, None),
            changelist_entry("b.txt", None, None),
            changelist_entry("c.txt", None, None),
            changelist_entry("d.txt", None, None),
            changelist_entry("p.txt", None, Some(&selection)),
            changelist_entry("../outside.txt", None, None),
        ];
        let mut reader = Reader::new(repo.path());

        let read = changelist_contents_with(&mut reader, &entries);

        assert_eq!(reader.adds, 1);
        assert_eq!(read.len(), entries.len());
        for (entry, got) in entries.iter().zip(&read) {
            assert_eq!(
                got,
                &source(&mut Reader::new(repo.path()), entry),
                "{}",
                entry.path
            );
        }
        assert_eq!(
            read[0]
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .after
                .as_ref()
                .unwrap()
                .1,
            b"two\n"
        );
        assert!(read[5].is_err());
    }

    // `rev-parse -q --verify <x>^{commit}` exits with 1 for an unknown object and for one that is
    // not a commit; anything else is not about the hash.
    #[test]
    fn only_an_unknown_object_is_not_a_commit() {
        let failed = |code| GitError::Failed {
            args: Vec::new(),
            code,
            stdout: String::new(),
            stderr: String::new(),
        };

        assert_eq!(
            not_a_commit("abc", failed(Some(1))),
            failure("abc is not a commit")
        );
        assert_eq!(
            not_a_commit("abc", failed(Some(128))),
            PatchError::Git(failed(Some(128)))
        );
        assert_eq!(
            not_a_commit("abc", GitError::NotInstalled),
            PatchError::Git(GitError::NotInstalled)
        );
    }

    #[test]
    fn paths_out_of_the_repository_or_into_git_are_refused() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\n", "a");
        for path in ["../a.txt", ".git/config", "/etc/hosts", ""] {
            assert!(sources(repo.path(), &[local(path)]).is_err(), "{path}");
        }
        let renamed = PatchEntry {
            old_path: Some("../a.txt".to_string()),
            ..local("a.txt")
        };
        assert!(sources(repo.path(), &[renamed]).is_err());
        let disk_before = PatchEntry {
            before: Some(Version::Disk),
            ..local("a.txt")
        };
        assert!(sources(repo.path(), &[disk_before]).is_err());
    }

    #[test]
    fn submodules_and_unchanged_files_give_nothing() {
        let repo = TempRepo::new();
        let head = repo.commit("a.txt", b"one\n", "a");
        repo.git(&[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{head},sub"),
        ]);
        repo.git(&["commit", "-q", "-m", "sub"]);

        let patch = text(
            repo.path(),
            &[local("a.txt"), local("sub")],
            &CreateOptions::default(),
        )
        .unwrap();

        assert_eq!(patch, b"");
    }

    #[test]
    fn a_binary_entry_git_prints_as_text_is_refused_with_its_name() {
        let repo = TempRepo::new();
        repo.commit(".gitattributes", b"* diff\n", "attributes");
        repo.commit("image.bin", b"\0one\r\n", "image");
        fs::write(repo.path().join("image.bin"), b"\0two\r\n").unwrap();

        let error = text(
            repo.path(),
            &[local("image.bin")],
            &CreateOptions::default(),
        )
        .unwrap_err();

        assert_eq!(
            error,
            PatchError::BinaryAsText {
                path: "image.bin".to_string()
            }
        );
    }

    #[test]
    fn the_message_reverse_and_base_path_reach_the_text() {
        let repo = TempRepo::new();
        repo.commit("dir/a.txt", b"one\n", "a");
        fs::write(repo.path().join("dir/a.txt"), b"two\n").unwrap();
        let base = repo.path().join("dir").display().to_string();
        let options = CreateOptions {
            base_path: base,
            reverse: true,
            message: Some("Fix it\n\nBody".to_string()),
        };

        let patch =
            String::from_utf8(text(repo.path(), &[local("dir/a.txt")], &options).unwrap()).unwrap();

        assert!(
            patch.starts_with("Subject: [PATCH] Fix it\n\nBody\n---\nIndex: a.txt\n"),
            "{patch}"
        );
        assert!(patch.contains("\n-two\n+one\n"), "{patch}");
        let relative = CreateOptions {
            base_path: "dir".to_string(),
            ..CreateOptions::default()
        };
        assert!(text(repo.path(), &[local("dir/a.txt")], &relative).is_err());
    }

    #[test]
    fn writing_a_file_makes_its_folder_and_replaces_the_old_one() {
        let dir = tempfile::TempDir::new().unwrap();
        let file = dir.path().join("new/folder/x.patch");

        write_file(&file, b"one").unwrap();
        write_file(&file, b"two").unwrap();

        assert_eq!(fs::read(&file).unwrap(), b"two");
        assert!(write_file(Path::new("relative.patch"), b"x").is_err());
        assert!(write_file(dir.path(), b"x").is_err());
    }
}
