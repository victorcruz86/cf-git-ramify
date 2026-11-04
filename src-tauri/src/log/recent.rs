use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::Mutex;

use crate::storage;

const FILE_NAME: &str = "log-filters.json";
// `VcsLogProjectTabsProperties.RECENTLY_FILTERED_VALUES_LIMIT`.
const LIMIT: usize = 10;
// The history of `SearchTextField`.
const TEXT_LIMIT: usize = 5;
const TEXT: &str = "Text";

// Two consecutive filters must not overwrite each other: each one reads and rewrites the whole
// file.
static WRITING: Mutex<()> = Mutex::new(());

// Per filter, from newest to oldest.
pub type RecentFilters = BTreeMap<String, Vec<Vec<String>>>;

// They are a convenience: if they cannot be read, it starts from scratch.
pub fn all(dir: &Path) -> RecentFilters {
    fs::read(dir.join(FILE_NAME))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn remember(dir: &Path, filter: &str, values: &[String]) -> io::Result<RecentFilters> {
    let _writing = WRITING.lock().unwrap();
    let mut recent = all(dir);
    let groups = recent.entry(filter.to_string()).or_default();
    groups.retain(|group| group != values);
    groups.insert(0, values.to_vec());
    groups.truncate(if filter == TEXT { TEXT_LIMIT } else { LIMIT });
    fs::create_dir_all(dir)?;
    let json = serde_json::to_vec_pretty(&recent).map_err(io::Error::other)?;
    storage::write_atomic(&dir.join(FILE_NAME), &json)?;
    Ok(recent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn values(text: &str) -> Vec<String> {
        vec![text.to_string()]
    }

    #[test]
    fn without_file_there_are_no_recent_filters() {
        let dir = TempDir::new().unwrap();

        assert!(all(dir.path()).is_empty());
    }

    #[test]
    fn the_latest_goes_first_and_is_not_repeated() {
        let dir = TempDir::new().unwrap();
        remember(dir.path(), "Branch", &values("a")).unwrap();
        remember(dir.path(), "Branch", &values("b")).unwrap();

        let recent = remember(dir.path(), "Branch", &values("a")).unwrap();

        assert_eq!(recent["Branch"], [values("a"), values("b")]);
        assert_eq!(all(dir.path())["Branch"], [values("a"), values("b")]);
    }

    #[test]
    fn each_filter_keeps_ten_and_the_text_five() {
        let dir = TempDir::new().unwrap();
        for index in 0..12 {
            remember(dir.path(), "Branch", &values(&index.to_string())).unwrap();
            remember(dir.path(), "Text", &values(&index.to_string())).unwrap();
        }

        let recent = all(dir.path());

        assert_eq!(recent["Branch"].len(), 10);
        assert_eq!(recent["Branch"][0], values("11"));
        assert_eq!(recent["Text"].len(), 5);
    }

    #[test]
    fn damaged_file_does_not_prevent_working() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(FILE_NAME), "not json").unwrap();

        assert!(all(dir.path()).is_empty());
        let recent = remember(dir.path(), "User", &values("Alice")).unwrap();
        assert_eq!(recent["User"], [values("Alice")]);
    }
}
