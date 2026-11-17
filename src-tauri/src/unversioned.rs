// `$Delete` of the Local Changes tab for unversioned files (`ChangesListView` gives
// `VirtualFileDeleteProvider`). IntelliJ deletes the files and keeps them in its Local History;
// Ramify has none, so they go to the system Trash, from where they can be brought back
// (`docs/parity-matrix.md`). Only unversioned files are taken: a versioned one, a folder, an ignored
// file or anything git does not list as unversioned refuses the whole request before anything moves.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Path;

use crate::git::{self, GitError};
use crate::snapshots;

fn invalid(error: io::Error) -> GitError {
    GitError::Io(error.to_string())
}

// The files of `paths` that git lists as unversioned, with the user's ignore rules applied.
pub(crate) fn unversioned(repo: &Path, paths: &[String]) -> Result<BTreeSet<String>, GitError> {
    let mut found = BTreeSet::new();
    for chunk in paths.chunks(git::PATHS_PER_COMMAND) {
        let mut args = vec![
            "--literal-pathspecs",
            "ls-files",
            "-z",
            "--others",
            "--exclude-standard",
            "--",
        ];
        args.extend(chunk.iter().map(String::as_str));
        let output = git::run(repo, &args)?;
        for record in output.split(|byte| *byte == 0).filter(|record| !record.is_empty()) {
            found.insert(String::from_utf8_lossy(record).into_owned());
        }
    }
    Ok(found)
}

// The folders the files were in, up to the root, while they are left empty: git does not list an
// empty folder, so it would stay behind unseen.
fn remove_empty_parents(repo: &Path, path: &str) {
    let mut folder = Path::new(path).parent();
    while let Some(relative) = folder.filter(|relative| !relative.as_os_str().is_empty()) {
        if fs::remove_dir(repo.join(relative)).is_err() {
            return;
        }
        folder = relative.parent();
    }
}

// Moves the files to the Trash with `trash`, files deeper in first (`FileComparator`: by path,
// descending). Returns the names of those that could not be moved, for `Could not erase files or
// folders:`; the others are gone from the working tree.
pub fn delete(
    repo: &Path,
    paths: &[String],
    trash: &mut dyn FnMut(&Path) -> io::Result<()>,
) -> Result<Vec<String>, GitError> {
    for path in paths {
        snapshots::check_path(repo, path).map_err(invalid)?;
    }
    let listed = unversioned(repo, paths)?;
    if let Some(path) = paths.iter().find(|path| !listed.contains(*path)) {
        return Err(GitError::Io(format!("\"{path}\" is not an unversioned file")));
    }
    let mut sorted: Vec<&String> = paths.iter().collect::<BTreeSet<_>>().into_iter().collect();
    sorted.reverse();
    let mut problems = Vec::new();
    for path in sorted {
        let target = repo.join(path);
        match trash(&target) {
            Ok(()) => remove_empty_parents(repo, path),
            Err(_) => problems.push(
                Path::new(path)
                    .file_name()
                    .map_or_else(|| path.clone(), |name| name.to_string_lossy().into_owned()),
            ),
        }
    }
    Ok(problems)
}

// The system Trash. On macOS with `NSFileManager`: the default asks Finder through AppleScript,
// which needs a permission and plays Finder's sound.
pub fn to_trash(path: &Path) -> io::Result<()> {
    #[allow(unused_mut)]
    let mut context = trash::TrashContext::default();
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        context.set_delete_method(DeleteMethod::NsFileManager);
    }
    context.delete(path).map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempRepo;
    use tempfile::TempDir;

    // A Trash of its own: the tests never touch the user's.
    struct FakeTrash {
        bin: TempDir,
        moved: Vec<String>,
    }

    impl FakeTrash {
        fn new() -> Self {
            Self {
                bin: TempDir::new().unwrap(),
                moved: Vec::new(),
            }
        }

        fn take(&mut self, path: &Path) -> io::Result<()> {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            fs::rename(path, self.bin.path().join(format!("{}-{name}", self.moved.len())))?;
            self.moved.push(name);
            Ok(())
        }
    }

    fn repo() -> TempRepo {
        let repo = TempRepo::new();
        repo.commit("tracked.txt", b"one\n", "first");
        fs::write(repo.path().join(".gitignore"), "ignored.log\n").unwrap();
        repo.git(&["add", ".gitignore"]);
        repo.git(&["commit", "-q", "-m", "ignore"]);
        repo
    }

    fn write(repo: &TempRepo, path: &str) {
        let target = repo.path().join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, "new\n").unwrap();
    }

    fn paths(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn unversioned_files_go_to_the_trash_deepest_first_and_empty_folders_go_too() {
        let repo = repo();
        write(&repo, "notes.md");
        write(&repo, "docs/spike/a.md");
        write(&repo, "docs/spike/b.md");
        let mut bin = FakeTrash::new();

        let problems = delete(
            repo.path(),
            &paths(&["notes.md", "docs/spike/a.md", "docs/spike/b.md"]),
            &mut |path| bin.take(path),
        )
        .unwrap();

        assert!(problems.is_empty());
        assert_eq!(bin.moved, ["notes.md", "b.md", "a.md"]);
        assert!(!repo.path().join("docs").exists());
        assert_eq!(fs::read_to_string(repo.path().join("tracked.txt")).unwrap(), "one\n");
    }

    #[test]
    fn a_folder_that_keeps_other_files_stays() {
        let repo = repo();
        write(&repo, "docs/a.md");
        write(&repo, "docs/keep.md");
        let mut bin = FakeTrash::new();

        delete(repo.path(), &paths(&["docs/a.md"]), &mut |path| bin.take(path)).unwrap();

        assert!(repo.path().join("docs/keep.md").exists());
    }

    #[test]
    fn anything_but_an_unversioned_file_refuses_the_whole_request() {
        let repo = repo();
        write(&repo, "new.md");
        write(&repo, "ignored.log");
        write(&repo, "folder/inside.md");
        fs::write(repo.path().join("tracked.txt"), "changed\n").unwrap();
        for bad in ["tracked.txt", "ignored.log", "folder", "missing.md", "../outside.md", ".git/config"] {
            let mut bin = FakeTrash::new();

            let result = delete(repo.path(), &paths(&["new.md", bad]), &mut |path| bin.take(path));

            assert!(result.is_err(), "{bad} was accepted");
            assert!(bin.moved.is_empty(), "{bad}: something moved");
        }
        assert!(repo.path().join("new.md").exists());
    }

    #[test]
    fn a_file_the_trash_refuses_is_named_and_the_others_still_go() {
        let repo = repo();
        write(&repo, "a.md");
        write(&repo, "b.md");
        let mut bin = FakeTrash::new();

        let problems = delete(repo.path(), &paths(&["a.md", "b.md"]), &mut |path| {
            if path.ends_with("b.md") {
                Err(io::Error::other("locked"))
            } else {
                bin.take(path)
            }
        })
        .unwrap();

        assert_eq!(problems, ["b.md"]);
        assert_eq!(bin.moved, ["a.md"]);
        assert!(repo.path().join("b.md").exists());
    }

    #[test]
    fn names_with_spaces_and_glob_characters_are_taken_literally() {
        let repo = repo();
        write(&repo, "my notes [draft].md");
        write(&repo, "my notes d.md");
        let mut bin = FakeTrash::new();

        delete(repo.path(), &paths(&["my notes [draft].md"]), &mut |path| bin.take(path)).unwrap();

        assert_eq!(bin.moved, ["my notes [draft].md"]);
        assert!(repo.path().join("my notes d.md").exists());
    }
}
