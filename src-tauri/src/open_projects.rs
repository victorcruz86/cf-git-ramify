// The projects open in this process's windows, for `Reopen projects on startup`: a snapshot of the
// main windows that have a repository, in the order the windows were opened, and the one focused
// last. It is written whenever that changes and never on quit, so it holds what was open at quit,
// even after a crash (IntelliJ's `opened` flag of `RecentProjectsManagerBase`).

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::storage;

pub const FILE: &str = "open-projects.json";
const CORRUPT: &str = "open-projects.damaged.json";

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenProjects {
    #[serde(default)]
    pub projects: Vec<String>,
    #[serde(default)]
    pub last_focused: Option<String>,
}

// The list in `dir`. A missing file is an empty list; a damaged one is set aside.
pub fn load(dir: &Path) -> io::Result<OpenProjects> {
    storage::load_json(dir, FILE, CORRUPT)
}

// The snapshot is this process's alone: it replaces the file as a whole, never half written.
pub fn save(dir: &Path, open: &OpenProjects) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let bytes = serde_json::to_vec_pretty(open)?;
    storage::write_atomic(&dir.join(FILE), &bytes)
}

// `sessions_in_open_order` holds each main window's label and repository, in the order the
// windows were opened; `focused` is the label of the main window focused last. A repository is
// listed once, where it first appears.
pub fn snapshot(
    sessions_in_open_order: &[(String, String)],
    focused: Option<&str>,
) -> OpenProjects {
    let mut projects: Vec<String> = Vec::new();
    for (_, repo) in sessions_in_open_order {
        if !projects.contains(repo) {
            projects.push(repo.clone());
        }
    }
    let last_focused = focused.and_then(|focused| {
        sessions_in_open_order
            .iter()
            .find(|(label, _)| label == focused)
            .map(|(_, repo)| repo.clone())
    });
    OpenProjects {
        projects,
        last_focused,
    }
}

// What a start reopens: `first` goes in the initial window and `rest` in new windows, in order;
// `missing` are the paths that no longer exist, which are skipped and taken out of the list.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reopen {
    pub first: Option<String>,
    pub rest: Vec<String>,
    pub missing: Vec<String>,
    pub last_focused: Option<String>,
}

impl Reopen {
    // The list without the missing paths, as it is saved again at start.
    pub fn kept(&self) -> OpenProjects {
        OpenProjects {
            projects: self.first.iter().chain(&self.rest).cloned().collect(),
            last_focused: self.last_focused.clone(),
        }
    }
}

pub fn plan(open: &OpenProjects, exists: impl Fn(&str) -> bool) -> Reopen {
    let (kept, missing): (Vec<String>, Vec<String>) =
        open.projects.iter().cloned().partition(|path| exists(path));
    let last_focused = open
        .last_focused
        .clone()
        .filter(|focused| kept.contains(focused));
    let mut kept = kept.into_iter();
    Reopen {
        first: kept.next(),
        rest: kept.collect(),
        missing,
        last_focused,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;
    use tempfile::TempDir;

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(label, repo)| (label.to_string(), repo.to_string()))
            .collect()
    }

    fn paths(list: &[&str]) -> Vec<String> {
        list.iter().map(|path| path.to_string()).collect()
    }

    #[test]
    fn the_snapshot_keeps_the_opening_order_and_the_last_focused() {
        // `main-3` was opened before `main-2` got its repository by This Window: the order is
        // the windows', not the labels' nor the opens'.
        let open = pairs(&[
            ("main", "/work/a"),
            ("main-3", "/work/c"),
            ("main-2", "/work/b"),
        ]);

        let snapshot = snapshot(&open, Some("main-3"));

        assert_eq!(snapshot.projects, paths(&["/work/a", "/work/c", "/work/b"]));
        assert_eq!(snapshot.last_focused.as_deref(), Some("/work/c"));
        // A focused window without a repository names none.
        assert_eq!(super::snapshot(&open, Some("main-4")).last_focused, None);
        assert_eq!(super::snapshot(&open, None).last_focused, None);
    }

    #[test]
    fn a_damaged_file_is_set_aside() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(FILE), b"{ not json").unwrap();

        assert_eq!(load(dir.path()).unwrap(), OpenProjects::default());

        let kept = fs::read(dir.path().join("open-projects.damaged.json")).unwrap();
        assert_eq!(kept, b"{ not json");
        assert!(!dir.path().join(FILE).exists());
    }

    #[test]
    fn saving_is_atomic() {
        let dir = TempDir::new().unwrap();
        let first = OpenProjects {
            projects: paths(&["/work/a"]),
            last_focused: Some("/work/a".to_string()),
        };
        save(dir.path(), &first).unwrap();
        let file = dir.path().join(FILE);
        let before = fs::metadata(&file).unwrap().ino();
        // A hard link to the old file: a write in place would change it too.
        let old = dir.path().join("old.json");
        fs::hard_link(&file, &old).unwrap();

        let second = OpenProjects {
            projects: paths(&["/work/a", "/work/b"]),
            last_focused: Some("/work/b".to_string()),
        };
        save(dir.path(), &second).unwrap();

        assert_ne!(
            fs::metadata(&file).unwrap().ino(),
            before,
            "it was replaced"
        );
        assert_eq!(
            serde_json::from_slice::<OpenProjects>(&fs::read(&old).unwrap()).unwrap(),
            first
        );
        assert_eq!(load(dir.path()).unwrap(), second);
        let mut names: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["old.json", FILE], "no temporary file is left");
    }

    #[test]
    fn the_file_is_read_in_the_shape_it_is_written() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join(FILE),
            br#"{"projects": ["/work/a"], "lastFocused": "/work/a"}"#,
        )
        .unwrap();

        let open = load(dir.path()).unwrap();

        assert_eq!(open.projects, paths(&["/work/a"]));
        assert_eq!(open.last_focused.as_deref(), Some("/work/a"));
    }

    #[test]
    fn reopening_skips_the_missing_paths() {
        let open = OpenProjects {
            projects: paths(&["/gone/x", "/work/a", "/gone/y", "/work/b"]),
            last_focused: Some("/work/b".to_string()),
        };

        let plan = plan(&open, |path| path.starts_with("/work"));

        assert_eq!(plan.first.as_deref(), Some("/work/a"));
        assert_eq!(plan.rest, paths(&["/work/b"]));
        assert_eq!(plan.missing, paths(&["/gone/x", "/gone/y"]));
        assert_eq!(plan.last_focused.as_deref(), Some("/work/b"));
        assert_eq!(plan.kept().projects, paths(&["/work/a", "/work/b"]));
    }

    #[test]
    fn a_missing_last_focused_is_forgotten() {
        let open = OpenProjects {
            projects: paths(&["/work/a", "/gone/x"]),
            last_focused: Some("/gone/x".to_string()),
        };

        let plan = plan(&open, |path| path.starts_with("/work"));

        assert_eq!(plan.last_focused, None);
        assert_eq!(plan.kept().last_focused, None);
    }
}
