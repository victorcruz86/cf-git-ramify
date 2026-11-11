// File > Recent Projects, after IntelliJ's `RecentProjectsManagerBase` and
// `RecentProjectListActionProvider`: the repositories opened in Ramify, the latest first.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::storage;

pub const FILE: &str = "recent-projects.json";
const CORRUPT: &str = "recent-projects.damaged.json";
// IntelliJ's advanced setting `ide.max.recent.projects`, 50 by default: past it the oldest goes.
pub const MAX: usize = 50;

#[derive(Debug, Default, Serialize, Deserialize)]
struct Stored {
    projects: Vec<String>,
}

// A menu row: the repository and the text it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub label: String,
}

// The list in `dir`, the latest first. A missing file is an empty list; a damaged one is set aside.
pub fn load(dir: &Path) -> io::Result<Vec<String>> {
    Ok(storage::load_json::<Stored>(dir, FILE, CORRUPT)?.projects)
}

// `markPathRecent`: the repository goes first. The file is read again right before writing, so
// what another Ramify window recorded stays.
pub fn record(dir: &Path, path: &str) -> io::Result<Vec<String>> {
    update(dir, |projects| {
        projects.retain(|known| known != path);
        projects.insert(0, path.to_string());
        projects.truncate(MAX);
    })
}

// `Remove From List` of the dialog for a path that no longer exists.
pub fn remove(dir: &Path, path: &str) -> io::Result<Vec<String>> {
    update(dir, |projects| projects.retain(|known| known != path))
}

fn update(dir: &Path, change: impl FnOnce(&mut Vec<String>)) -> io::Result<Vec<String>> {
    fs::create_dir_all(dir)?;
    let mut projects = load(dir)?;
    change(&mut projects);
    let bytes = serde_json::to_vec_pretty(&Stored {
        projects: projects.clone(),
    })?;
    storage::write_atomic(&dir.join(FILE), &bytes)?;
    Ok(projects)
}

// `RecentProjectsGroup`: without the window's own repository. A row shows the folder's name; when
// two repositories share a name, the open one included, each shows its path, from `~` if it is
// inside the home folder (`getLocationRelativeToUserHome`).
pub fn menu_entries(projects: &[String], current: Option<&str>, home: Option<&Path>) -> Vec<Entry> {
    let mut seen = HashSet::new();
    let mut names = HashSet::new();
    let mut duplicates = HashSet::new();
    for path in current
        .into_iter()
        .chain(projects.iter().map(String::as_str))
    {
        if seen.insert(path) && !names.insert(name(path)) {
            duplicates.insert(name(path));
        }
    }
    projects
        .iter()
        .filter(|path| Some(path.as_str()) != current)
        .map(|path| Entry {
            path: path.clone(),
            label: if duplicates.contains(&name(path)) {
                from_home(path, home)
            } else {
                name(path)
            },
        })
        .collect()
}

// IntelliJ's `ProjectToolbarWidgetAction.MAX_RECENT_COUNT`.
pub const MAX_WIDGET_RECENT: usize = 100;

// A row of the project widget's popup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRow {
    pub path: String,
    // The folder's name.
    pub name: String,
    // The path from `~` if it is inside the home folder (`getLocationRelativeToUserHome`).
    pub home_path: String,
    // Whether the folder is still there; a recent row that is gone asks to remove it instead.
    pub exists: bool,
}

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct ProjectsList {
    pub open: Vec<ProjectRow>,
    pub recent: Vec<ProjectRow>,
}

fn row(path: &str, home: Option<&Path>) -> ProjectRow {
    ProjectRow {
        path: path.to_string(),
        name: name(path),
        home_path: from_home(path, home),
        exists: Path::new(path).exists(),
    }
}

// Two spellings of one folder: equal, or the same canonical path.
fn same_project(a: &str, b: &str) -> bool {
    a == b
        || matches!(
            (fs::canonicalize(a), fs::canonicalize(b)),
            (Ok(a), Ok(b)) if a == b
        )
}

// The widget's popup (`ProjectToolbarWidgetAction.createActionGroup`) for the window `own`:
// `open` is the repositories of the other windows (`windows`: label and repository, in opening
// order); `recent` is the recent list without any open repository, the window's own included,
// at most `MAX_WIDGET_RECENT`.
pub fn projects_list(
    windows: &[(String, String)],
    own: &str,
    recent: &[String],
    home: Option<&Path>,
) -> ProjectsList {
    let open = windows
        .iter()
        .filter(|(label, _)| label != own)
        .map(|(_, path)| row(path, home))
        .collect();
    let recent = recent
        .iter()
        .filter(|path| !windows.iter().any(|(_, open)| same_project(open, path)))
        .take(MAX_WIDGET_RECENT)
        .map(|path| row(path, home))
        .collect();
    ProjectsList { open, recent }
}

fn name(path: &str) -> String {
    match Path::new(path).file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => path.to_string(),
    }
}

fn from_home(path: &str, home: Option<&Path>) -> String {
    match home.and_then(|home| Path::new(path).strip_prefix(home).ok()) {
        Some(rest) if !rest.as_os_str().is_empty() => format!("~/{}", rest.display()),
        _ => path.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn paths(list: &[&str]) -> Vec<String> {
        list.iter().map(|path| path.to_string()).collect()
    }

    #[test]
    fn recording_puts_the_project_first_without_repeating() {
        let dir = TempDir::new().unwrap();
        record(dir.path(), "/work/a").unwrap();
        record(dir.path(), "/work/b").unwrap();
        let list = record(dir.path(), "/work/a").unwrap();
        assert_eq!(list, paths(&["/work/a", "/work/b"]));
        assert_eq!(load(dir.path()).unwrap(), list);
    }

    #[test]
    fn the_list_keeps_fifty_and_drops_the_oldest() {
        let dir = TempDir::new().unwrap();
        for number in 0..=MAX {
            record(dir.path(), &format!("/work/p{number}")).unwrap();
        }
        let list = load(dir.path()).unwrap();
        assert_eq!(list.len(), 50);
        assert_eq!(list[0], "/work/p50");
        assert!(!list.contains(&"/work/p0".to_string()));
    }

    #[test]
    fn a_missing_file_is_an_empty_list_and_the_folder_is_created() {
        let dir = TempDir::new().unwrap();
        let data = dir.path().join("not-yet");
        assert!(load(&data).unwrap().is_empty());
        assert_eq!(record(&data, "/work/a").unwrap(), paths(&["/work/a"]));
    }

    #[test]
    fn a_damaged_file_is_set_aside_and_the_list_starts_again() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(FILE), b"{ not json").unwrap();
        assert_eq!(record(dir.path(), "/work/a").unwrap(), paths(&["/work/a"]));
        let kept = fs::read(dir.path().join("recent-projects.damaged.json")).unwrap();
        assert_eq!(kept, b"{ not json");
    }

    #[test]
    fn removing_takes_the_project_out() {
        let dir = TempDir::new().unwrap();
        record(dir.path(), "/work/a").unwrap();
        record(dir.path(), "/work/b").unwrap();
        assert_eq!(remove(dir.path(), "/work/a").unwrap(), paths(&["/work/b"]));
        assert_eq!(load(dir.path()).unwrap(), paths(&["/work/b"]));
    }

    #[test]
    fn the_current_project_is_not_in_the_menu() {
        let list = paths(&["/work/a", "/work/b"]);
        let entries = menu_entries(&list, Some("/work/a"), None);
        assert_eq!(
            entries,
            vec![Entry {
                path: "/work/b".into(),
                label: "b".into()
            }]
        );
    }

    #[test]
    fn the_same_name_shows_the_path_from_home() {
        let list = paths(&[
            "/Users/me/x/app",
            "/Users/me/y/app",
            "/opt/app",
            "/Users/me/tool",
        ]);
        let labels: Vec<String> = menu_entries(&list, None, Some(Path::new("/Users/me")))
            .into_iter()
            .map(|entry| entry.label)
            .collect();
        assert_eq!(labels, ["~/x/app", "~/y/app", "/opt/app", "tool"]);
    }

    #[test]
    fn the_open_project_counts_for_the_same_name() {
        let list = paths(&["/Users/me/y/app"]);
        let entries = menu_entries(&list, Some("/Users/me/x/app"), Some(Path::new("/Users/me")));
        assert_eq!(entries[0].label, "~/y/app");
        // The same path open and in the list is one project (IDEA-211955).
        let list = paths(&["/Users/me/x/app", "/Users/me/tool"]);
        let entries = menu_entries(&list, Some("/Users/me/x/app"), Some(Path::new("/Users/me")));
        assert_eq!(entries[0].label, "tool");
    }

    #[test]
    fn a_folder_that_only_shares_the_home_prefix_is_shown_whole() {
        let list = paths(&["/Users/meow/app", "/Users/me/app"]);
        let labels: Vec<String> = menu_entries(&list, None, Some(Path::new("/Users/me")))
            .into_iter()
            .map(|entry| entry.label)
            .collect();
        assert_eq!(labels, ["/Users/meow/app", "~/app"]);
    }

    // The project widget's popup: the other windows' repositories, then the recent list without
    // any open one, the caller's own included.
    #[test]
    fn the_projects_list_splits_open_from_recent() {
        let home = Path::new("/home/me");
        let open = vec![
            ("main".to_string(), "/home/me/work/a".to_string()),
            ("main-2".to_string(), "/home/me/work/b".to_string()),
            ("main-3".to_string(), "/srv/c".to_string()),
        ];
        let recent = paths(&[
            "/home/me/work/a",
            "/home/me/work/b",
            "/home/me/work/d",
            "/srv/c",
            "/srv/e",
        ]);
        let list = projects_list(&open, "main", &recent, Some(home));
        let row = |path: &str, name: &str, home_path: &str| ProjectRow {
            path: path.to_string(),
            name: name.to_string(),
            home_path: home_path.to_string(),
            // None of these folders is on disk.
            exists: false,
        };
        assert_eq!(
            list.open,
            [
                row("/home/me/work/b", "b", "~/work/b"),
                row("/srv/c", "c", "/srv/c"),
            ]
        );
        assert_eq!(
            list.recent,
            [
                row("/home/me/work/d", "d", "~/work/d"),
                row("/srv/e", "e", "/srv/e"),
            ]
        );
    }

    #[test]
    fn the_recent_part_of_the_projects_list_keeps_at_most_a_hundred() {
        let recent: Vec<String> = (0..150).map(|number| format!("/work/p{number}")).collect();
        let list = projects_list(&[], "main", &recent, None);
        assert_eq!(list.recent.len(), MAX_WIDGET_RECENT);
        assert_eq!(list.recent[0].path, "/work/p0");
        assert!(list.open.is_empty());
    }

    #[test]
    fn a_path_equal_to_the_home_folder_or_outside_it_stays_whole() {
        let home = Path::new("/home/me");
        let list = projects_list(
            &[],
            "main",
            &paths(&["/home/me", "/home/meow/x"]),
            Some(home),
        );
        assert_eq!(list.recent[0].home_path, "/home/me");
        assert_eq!(list.recent[1].home_path, "/home/meow/x");
    }

    // A recent row says whether its folder still exists, so the widget can ask about a missing one.
    #[test]
    fn a_missing_recent_project_is_reported_as_missing() {
        let dir = TempDir::new().unwrap();
        let there = dir.path().join("there");
        fs::create_dir(&there).unwrap();
        let gone = dir.path().join("gone");
        let recent = paths(&[there.to_str().unwrap(), gone.to_str().unwrap()]);
        let list = projects_list(&[], "main", &recent, None);
        assert!(list.recent[0].exists);
        assert!(!list.recent[1].exists);
    }
}
