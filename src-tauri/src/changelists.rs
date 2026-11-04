use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::idea_import::ImportedLists;
use crate::storage;

pub const DEFAULT_LIST_NAME: &str = "Changes";
const DEFAULT_LIST_ID: &str = "default";
const FILE_NAME: &str = "changelists.json";
const CORRUPT_FILE_NAME: &str = "changelists.json.corrupt";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ChangeList {
    pub id: String,
    pub name: String,
    pub comment: String,
}

// Fields missing when reading take their default value, so fields can be added without breaking
// the files already saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Changelists {
    pub lists: Vec<ChangeList>,
    pub active: String,
    pub assignments: BTreeMap<String, String>,
    // Per file, the list of each known hunk (the ones already read with
    // `reconcile_hunks`). A hunk without an entry has not been seen yet and follows the file's list.
    pub hunk_assignments: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangelistError {
    EmptyName,
    DuplicateName(String),
    NotFound,
    LastList,
    HunkNotFound,
}

impl fmt::Display for ChangelistError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyName => write!(f, "The changelist name cannot be empty"),
            Self::DuplicateName(name) => write!(f, "A changelist named '{name}' already exists"),
            Self::NotFound => write!(f, "The changelist does not exist"),
            Self::LastList => write!(f, "The last changelist cannot be removed"),
            Self::HunkNotFound => write!(f, "The change no longer exists: the file has changed"),
        }
    }
}

impl Default for Changelists {
    fn default() -> Self {
        Changelists {
            lists: vec![ChangeList {
                id: DEFAULT_LIST_ID.to_string(),
                name: DEFAULT_LIST_NAME.to_string(),
                comment: String::new(),
            }],
            active: DEFAULT_LIST_ID.to_string(),
            assignments: BTreeMap::new(),
            hunk_assignments: BTreeMap::new(),
        }
    }
}

// Unique within a process by the counter and across processes by the time.
fn new_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    format!("cl-{nanos:x}-{:x}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

impl Changelists {
    // For whoever only displays: on any error it gives the initial list without touching the file.
    // Whoever is going to save must use `try_load`, or it would save over data it could not read.
    pub fn load(dir: &Path) -> Changelists {
        Changelists::try_load(dir).unwrap_or_default()
    }

    // A missing file gives the initial list. One that is not valid JSON is set aside as
    // `changelists.json.corrupt` (or `.corrupt.1`, `.2`... if there are others already) and it starts from scratch.
    // Any other failure, or not being able to keep the copy, is an error: it must not continue.
    pub fn try_load(dir: &Path) -> io::Result<Changelists> {
        let file = dir.join(FILE_NAME);
        let bytes = match fs::read(&file) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Changelists::default())
            }
            Err(error) => return Err(error),
        };
        match serde_json::from_slice::<Changelists>(&bytes) {
            Ok(mut lists) => {
                lists.normalize();
                Ok(lists)
            }
            Err(_) => {
                storage::keep_corrupt(dir, &file, CORRUPT_FILE_NAME)?;
                Ok(Changelists::default())
            }
        }
    }

    pub fn save(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        let json = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        storage::write_atomic(&dir.join(FILE_NAME), &json)
    }

    // Guarantees there is at least one list and that the active one exists.
    fn normalize(&mut self) {
        if self.lists.is_empty() {
            let initial = Changelists::default();
            self.lists = initial.lists;
            self.active = initial.active;
        } else if !self.has(&self.active) {
            self.active = self.lists[0].id.clone();
        }
    }

    fn has(&self, id: &str) -> bool {
        self.lists.iter().any(|list| list.id == id)
    }

    // A name without extra spaces, that is not empty and that no other list has.
    fn valid_name(&self, name: &str, except: Option<&str>) -> Result<String, ChangelistError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(ChangelistError::EmptyName);
        }
        let taken = self
            .lists
            .iter()
            .any(|list| list.name == name && Some(list.id.as_str()) != except);
        if taken {
            return Err(ChangelistError::DuplicateName(name.to_string()));
        }
        Ok(name.to_string())
    }

    pub fn create(&mut self, name: &str) -> Result<String, ChangelistError> {
        let name = self.valid_name(name, None)?;
        let id = new_id();
        self.lists.push(ChangeList {
            id: id.clone(),
            name,
            comment: String::new(),
        });
        Ok(id)
    }

    pub fn rename(&mut self, id: &str, name: &str) -> Result<(), ChangelistError> {
        if !self.has(id) {
            return Err(ChangelistError::NotFound);
        }
        let name = self.valid_name(name, Some(id))?;
        if let Some(list) = self.lists.iter_mut().find(|list| list.id == id) {
            list.name = name;
        }
        Ok(())
    }

    // The comment is the draft of the commit message that the Commit Changes dialog keeps for the list.
    pub fn set_comment(&mut self, id: &str, comment: &str) -> Result<(), ChangelistError> {
        let list = self
            .lists
            .iter_mut()
            .find(|list| list.id == id)
            .ok_or(ChangelistError::NotFound)?;
        list.comment = comment.to_string();
        Ok(())
    }

    // Its files move to the active list; if the active one is removed, to the first one left.
    pub fn remove(&mut self, id: &str) -> Result<(), ChangelistError> {
        if !self.has(id) {
            return Err(ChangelistError::NotFound);
        }
        if self.lists.len() == 1 {
            return Err(ChangelistError::LastList);
        }
        self.lists.retain(|list| list.id != id);
        if self.active == id {
            self.active = self.lists[0].id.clone();
        }
        for assigned in self
            .assignments
            .values_mut()
            .filter(|assigned| *assigned == id)
        {
            *assigned = self.active.clone();
        }
        for assigned in self
            .hunk_assignments
            .values_mut()
            .flat_map(BTreeMap::values_mut)
            .filter(|assigned| *assigned == id)
        {
            *assigned = self.active.clone();
        }
        Ok(())
    }

    pub fn set_active(&mut self, id: &str) -> Result<(), ChangelistError> {
        if !self.has(id) {
            return Err(ChangelistError::NotFound);
        }
        self.active = id.to_string();
        Ok(())
    }

    pub fn move_paths(&mut self, paths: &[String], id: &str) -> Result<(), ChangelistError> {
        if !self.has(id) {
            return Err(ChangelistError::NotFound);
        }
        for path in paths {
            self.assignments.insert(path.clone(), id.to_string());
            // Moving the whole file takes all its hunks with it.
            if let Some(hunks) = self.hunk_assignments.get_mut(path) {
                hunks
                    .values_mut()
                    .for_each(|assigned| *assigned = id.to_string());
            }
        }
        Ok(())
    }

    // Forgets the files that no longer have changes and assigns to the active list the new ones (and
    // the ones that pointed to a list that no longer exists). Returns whether anything changed, so that its
    // caller saves only when needed.
    pub fn reconcile(&mut self, paths: &[String]) -> bool {
        let current: BTreeSet<&str> = paths.iter().map(String::as_str).collect();
        let before = self.assignments.len();
        self.assignments
            .retain(|path, _| current.contains(path.as_str()));
        let mut changed = self.assignments.len() != before;
        let hunk_files = self.hunk_assignments.len();
        self.hunk_assignments
            .retain(|path, _| current.contains(path.as_str()));
        changed |= self.hunk_assignments.len() != hunk_files;

        for path in paths {
            let valid = self
                .assignments
                .get(path)
                .is_some_and(|id| self.lists.iter().any(|list| list.id == *id));
            if !valid {
                self.assignments.insert(path.clone(), self.active.clone());
                changed = true;
            }
        }
        changed
    }

    // Moves already known hunks of the file to another list. All or nothing: with a list or a hunk that
    // does not exist nothing is changed.
    pub fn move_hunks(
        &mut self,
        path: &str,
        hunks: &[String],
        id: &str,
    ) -> Result<(), ChangelistError> {
        if !self.has(id) {
            return Err(ChangelistError::NotFound);
        }
        let known = self.hunk_assignments.get_mut(path);
        let Some(known) = known.filter(|known| hunks.iter().all(|hunk| known.contains_key(hunk)))
        else {
            return Err(ChangelistError::HunkNotFound);
        };
        for hunk in hunks {
            known.insert(hunk.clone(), id.to_string());
        }
        Ok(())
    }

    // The file's hunks that are in the list, including the ones never moved if it is the
    // file's list. It only counts those seen in the last `reconcile_hunks`; in id order.
    pub fn hunks_in(&self, path: &str, id: &str) -> Vec<String> {
        self.hunk_assignments
            .get(path)
            .map_or_else(Vec::new, |known| {
                known
                    .iter()
                    .filter(|(_, assigned)| *assigned == id)
                    .map(|(hunk, _)| hunk.clone())
                    .collect()
            })
    }

    // Leaves only the hunks that exist now: one that changed is another hunk, and goes back to the file's
    // list, like a new one or one whose list no longer exists. Returns whether anything changed.
    pub fn reconcile_hunks(&mut self, path: &str, current_ids: &[String]) -> bool {
        let own = self.list_for(path).to_string();
        let before = self.hunk_assignments.get(path);
        let next: BTreeMap<String, String> = current_ids
            .iter()
            .map(|hunk| {
                let assigned = before
                    .and_then(|known| known.get(hunk))
                    .filter(|assigned| self.has(assigned))
                    .unwrap_or(&own);
                (hunk.clone(), assigned.clone())
            })
            .collect();
        if before == Some(&next) || (before.is_none() && next.is_empty()) {
            return false;
        }
        if next.is_empty() {
            self.hunk_assignments.remove(path);
        } else {
            self.hunk_assignments.insert(path.to_string(), next);
        }
        true
    }

    // Matching by name: the list that already exists keeps its id and its comment, and the
    // paths move to the list where IntelliJ has them. IntelliJ's active list is only adopted if
    // our own active one had no files assigned yet (before importing); otherwise ours is respected.
    pub fn merge_import(&mut self, imported: ImportedLists) {
        let active_in_use = self.assignments.values().any(|id| *id == self.active);
        let mut imported_active = None;
        for list in imported.lists {
            let existing = self.lists.iter().find(|own| own.name == list.name);
            let id = match existing {
                Some(own) => own.id.clone(),
                None => {
                    let id = new_id();
                    self.lists.push(ChangeList {
                        id: id.clone(),
                        name: list.name,
                        comment: list.comment,
                    });
                    id
                }
            };
            for path in list.paths {
                self.assignments.insert(path, id.clone());
            }
            if list.active {
                imported_active = Some(id);
            }
        }
        if let (false, Some(id)) = (active_in_use, imported_active) {
            self.active = id;
        }
    }

    // Identifier of the file's list; the ones not reconciled yet, in the active one.
    pub fn list_for(&self, path: &str) -> &str {
        self.assignments.get(path).map_or(&self.active, |id| id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn paths(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn id_of(lists: &Changelists, name: &str) -> String {
        lists
            .lists
            .iter()
            .find(|list| list.name == name)
            .unwrap()
            .id
            .clone()
    }

    #[test]
    fn initial_list() {
        let dir = TempDir::new().unwrap();

        let lists = Changelists::load(dir.path());

        assert_eq!(lists.lists.len(), 1);
        assert_eq!(lists.lists[0].name, DEFAULT_LIST_NAME);
        assert_eq!(lists.active, lists.lists[0].id);
        assert!(lists.assignments.is_empty());
    }

    #[test]
    fn new_file_goes_to_the_active_one() {
        let mut lists = Changelists::default();

        let changed = lists.reconcile(&paths(&["a.txt"]));

        assert!(changed);
        assert_eq!(lists.list_for("a.txt"), lists.active);
        assert_eq!(lists.assignments["a.txt"], lists.active);
    }

    #[test]
    fn new_file_goes_to_the_active_one_even_if_it_is_not_the_first() {
        let mut lists = Changelists::default();
        let other = lists.create("Other").unwrap();
        lists.set_active(&other).unwrap();

        lists.reconcile(&paths(&["a.txt"]));

        assert_eq!(lists.list_for("a.txt"), other);
    }

    #[test]
    fn file_that_disappears_is_forgotten() {
        let mut lists = Changelists::default();
        lists.reconcile(&paths(&["a.txt", "b.txt"]));

        let changed = lists.reconcile(&paths(&["b.txt"]));

        assert!(changed);
        assert!(!lists.assignments.contains_key("a.txt"));
        assert!(lists.assignments.contains_key("b.txt"));
    }

    #[test]
    fn reconciling_without_changes_reports_no_change() {
        let mut lists = Changelists::default();
        lists.reconcile(&paths(&["a.txt", "b.txt"]));
        let before = lists.clone();

        assert!(!lists.reconcile(&paths(&["b.txt", "a.txt"])));
        assert_eq!(lists, before);
    }

    #[test]
    fn reconciling_respects_the_assigned_list() {
        let mut lists = Changelists::default();
        let other = lists.create("Other").unwrap();
        lists.reconcile(&paths(&["a.txt"]));
        lists.move_paths(&paths(&["a.txt"]), &other).unwrap();

        assert!(!lists.reconcile(&paths(&["a.txt"])));
        assert_eq!(lists.list_for("a.txt"), other);
    }

    #[test]
    fn unassigned_belongs_to_the_active_one() {
        let lists = Changelists::default();
        assert_eq!(lists.list_for("new.txt"), lists.active);
    }

    #[test]
    fn create_list() {
        let mut lists = Changelists::default();

        let id = lists.create("  Experiment ").unwrap();

        assert_eq!(lists.lists.len(), 2);
        assert_eq!(lists.lists[1].id, id);
        assert_eq!(lists.lists[1].name, "Experiment");
        assert_eq!(lists.lists[1].comment, "");
        assert_ne!(id, lists.lists[0].id);
    }

    #[test]
    fn distinct_ids_when_creating_several() {
        let mut lists = Changelists::default();
        let ids: Vec<_> = (0..20)
            .map(|n| lists.create(&format!("L{n}")).unwrap())
            .collect();

        let mut unique = ids.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), ids.len());
    }

    #[test]
    fn empty_name_is_rejected() {
        let mut lists = Changelists::default();
        let before = lists.clone();

        assert_eq!(lists.create(""), Err(ChangelistError::EmptyName));
        assert_eq!(lists.create("  \t\n"), Err(ChangelistError::EmptyName));
        let id = lists.lists[0].id.clone();
        assert_eq!(lists.rename(&id, "   "), Err(ChangelistError::EmptyName));
        assert_eq!(lists, before);
    }

    #[test]
    fn duplicate_name_is_rejected() {
        let mut lists = Changelists::default();
        lists.create("Other").unwrap();

        assert_eq!(
            lists.create(" Other "),
            Err(ChangelistError::DuplicateName("Other".to_string()))
        );
        assert_eq!(
            lists.create(DEFAULT_LIST_NAME),
            Err(ChangelistError::DuplicateName(
                DEFAULT_LIST_NAME.to_string()
            ))
        );
        assert_eq!(lists.lists.len(), 2);
    }

    #[test]
    fn rename() {
        let mut lists = Changelists::default();
        let id = lists.create("Other").unwrap();

        lists.rename(&id, "New").unwrap();

        assert_eq!(id_of(&lists, "New"), id);
    }

    #[test]
    fn set_comment_keeps_name_and_id() {
        let mut lists = Changelists::default();
        let id = lists.create("Other").unwrap();

        lists.set_comment(&id, "work in progress").unwrap();

        let list = lists.lists.iter().find(|list| list.id == id).unwrap();
        assert_eq!(list.name, "Other");
        assert_eq!(list.comment, "work in progress");
        assert_eq!(lists.lists[0].comment, "");
        assert_eq!(
            lists.set_comment("missing", "x"),
            Err(ChangelistError::NotFound)
        );
    }

    #[test]
    fn renaming_to_another_lists_name_is_rejected() {
        let mut lists = Changelists::default();
        let id = lists.create("Other").unwrap();

        assert_eq!(
            lists.rename(&id, DEFAULT_LIST_NAME),
            Err(ChangelistError::DuplicateName(
                DEFAULT_LIST_NAME.to_string()
            ))
        );
        assert_eq!(id_of(&lists, "Other"), id);
    }

    #[test]
    fn renaming_with_its_own_name_is_valid() {
        let mut lists = Changelists::default();
        let id = lists.create("Other").unwrap();
        assert_eq!(lists.rename(&id, "Other"), Ok(()));
    }

    #[test]
    fn nonexistent_list() {
        let mut lists = Changelists::default();
        assert_eq!(lists.rename("nothing", "X"), Err(ChangelistError::NotFound));
        assert_eq!(lists.set_active("nothing"), Err(ChangelistError::NotFound));
        assert_eq!(lists.remove("nothing"), Err(ChangelistError::NotFound));
        assert_eq!(
            lists.move_paths(&paths(&["a.txt"]), "nothing"),
            Err(ChangelistError::NotFound)
        );
        assert!(lists.assignments.is_empty());
    }

    #[test]
    fn move_files() {
        let mut lists = Changelists::default();
        let other = lists.create("Other").unwrap();
        lists.reconcile(&paths(&["a.txt", "b.txt"]));

        lists.move_paths(&paths(&["a.txt"]), &other).unwrap();

        assert_eq!(lists.list_for("a.txt"), other);
        assert_eq!(lists.list_for("b.txt"), lists.active);
    }

    #[test]
    fn removing_a_list_moves_to_the_active_one() {
        let mut lists = Changelists::default();
        let other = lists.create("Other").unwrap();
        lists.reconcile(&paths(&["a.txt", "b.txt"]));
        lists.move_paths(&paths(&["a.txt"]), &other).unwrap();
        let active = lists.active.clone();

        lists.remove(&other).unwrap();

        assert_eq!(lists.lists.len(), 1);
        assert_eq!(lists.active, active);
        assert_eq!(lists.assignments["a.txt"], active);
        assert_eq!(lists.assignments["b.txt"], active);
    }

    #[test]
    fn removing_the_active_list() {
        let mut lists = Changelists::default();
        let first = lists.active.clone();
        let second = lists.create("Second").unwrap();
        lists.create("Third").unwrap();
        lists.set_active(&second).unwrap();
        lists.reconcile(&paths(&["a.txt"]));

        lists.remove(&second).unwrap();

        assert_eq!(lists.active, first);
        assert_eq!(lists.assignments["a.txt"], first);
    }

    #[test]
    fn the_last_one_is_not_removed() {
        let mut lists = Changelists::default();
        let id = lists.active.clone();

        assert_eq!(lists.remove(&id), Err(ChangelistError::LastList));
        assert_eq!(lists.lists.len(), 1);
    }

    #[test]
    fn saves_and_recovers() {
        let dir = TempDir::new().unwrap();
        let mut lists = Changelists::default();
        let other = lists.create("Other").unwrap();
        lists.reconcile(&paths(&["a.txt", "b.txt"]));
        lists.move_paths(&paths(&["b.txt"]), &other).unwrap();
        lists.set_active(&other).unwrap();

        lists.save(dir.path()).unwrap();

        assert_eq!(Changelists::load(dir.path()), lists);
    }

    #[test]
    fn saving_creates_the_folder() {
        let dir = TempDir::new().unwrap();
        let nested = dir.path().join("a").join("b");

        Changelists::default().save(&nested).unwrap();

        assert!(nested.join(FILE_NAME).is_file());
    }

    #[test]
    fn damaged_file() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(FILE_NAME), b"{ this is not json").unwrap();

        let lists = Changelists::load(dir.path());

        assert_eq!(lists, Changelists::default());
        assert_eq!(
            fs::read(dir.path().join(CORRUPT_FILE_NAME)).unwrap(),
            b"{ this is not json"
        );
        assert!(!dir.path().join(FILE_NAME).exists());
    }

    #[test]
    fn unknown_and_missing_fields_are_tolerated() {
        let dir = TempDir::new().unwrap();
        let json = r#"{
            "lists": [{"id": "x", "name": "Mine", "future": true}],
            "active": "x",
            "hunks": {"a.txt": [1, 2]}
        }"#;
        fs::write(dir.path().join(FILE_NAME), json).unwrap();

        let lists = Changelists::load(dir.path());

        assert_eq!(lists.lists.len(), 1);
        assert_eq!(lists.lists[0].name, "Mine");
        assert_eq!(lists.lists[0].comment, "");
        assert!(lists.assignments.is_empty());
        assert!(!dir.path().join(CORRUPT_FILE_NAME).exists());
    }

    #[test]
    fn file_with_no_fields_gives_the_initial_list() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(FILE_NAME), b"{}").unwrap();

        assert_eq!(Changelists::load(dir.path()), Changelists::default());
    }

    #[test]
    fn nonexistent_active_falls_back_to_the_first() {
        let dir = TempDir::new().unwrap();
        let json = r#"{"lists": [{"id": "x", "name": "Mine"}], "active": "deleted"}"#;
        fs::write(dir.path().join(FILE_NAME), json).unwrap();

        assert_eq!(Changelists::load(dir.path()).active, "x");
    }

    #[test]
    fn no_lists_in_the_file_gives_the_initial_list() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join(FILE_NAME),
            br#"{"lists": [], "active": "x"}"#,
        )
        .unwrap();

        let lists = Changelists::load(dir.path());

        assert_eq!(lists.lists.len(), 1);
        assert_eq!(lists.active, lists.lists[0].id);
    }

    #[test]
    fn assignment_to_nonexistent_list_is_reassigned() {
        let mut lists = Changelists::default();
        lists
            .assignments
            .insert("a.txt".to_string(), "deleted".to_string());

        assert!(lists.reconcile(&paths(&["a.txt"])));
        assert_eq!(lists.assignments["a.txt"], lists.active);
    }

    fn ids(names: &[&str]) -> Vec<String> {
        paths(names)
    }

    #[test]
    fn hunk_in_another_list() {
        let mut lists = Changelists::default();
        let other = lists.create("Other").unwrap();
        lists.reconcile(&paths(&["a.txt"]));
        lists.reconcile_hunks("a.txt", &ids(&["h1", "h2", "h3"]));

        lists.move_hunks("a.txt", &ids(&["h2"]), &other).unwrap();

        let own = lists.list_for("a.txt").to_string();
        assert_eq!(lists.hunks_in("a.txt", &other), ids(&["h2"]));
        assert_eq!(lists.hunks_in("a.txt", &own), ids(&["h1", "h3"]));
        assert_eq!(lists.list_for("a.txt"), own);
    }

    #[test]
    fn moved_hunk_that_disappears() {
        let mut lists = Changelists::default();
        let other = lists.create("Other").unwrap();
        lists.reconcile(&paths(&["a.txt"]));
        lists.reconcile_hunks("a.txt", &ids(&["h1", "h2"]));
        lists.move_hunks("a.txt", &ids(&["h2"]), &other).unwrap();

        // Devin changes the moved hunk: now it is another one.
        let changed = lists.reconcile_hunks("a.txt", &ids(&["h1", "h2-new"]));

        let own = lists.list_for("a.txt").to_string();
        assert!(changed);
        assert!(lists.hunks_in("a.txt", &other).is_empty());
        assert_eq!(lists.hunks_in("a.txt", &own), ids(&["h1", "h2-new"]));
        assert!(!lists.reconcile_hunks("a.txt", &ids(&["h1", "h2-new"])));
    }

    #[test]
    fn reconciling_hunks_keeps_what_was_moved_that_remains() {
        let mut lists = Changelists::default();
        let other = lists.create("Other").unwrap();
        lists.reconcile(&paths(&["a.txt"]));
        lists.reconcile_hunks("a.txt", &ids(&["h1", "h2"]));
        lists.move_hunks("a.txt", &ids(&["h2"]), &other).unwrap();

        assert!(lists.reconcile_hunks("a.txt", &ids(&["h1", "h2", "h3"])));

        assert_eq!(lists.hunks_in("a.txt", &other), ids(&["h2"]));
    }

    #[test]
    fn new_hunk_goes_to_the_files_list() {
        let mut lists = Changelists::default();
        let other = lists.create("Other").unwrap();
        lists.reconcile(&paths(&["a.txt"]));
        lists.move_paths(&paths(&["a.txt"]), &other).unwrap();

        lists.reconcile_hunks("a.txt", &ids(&["h1"]));

        assert_eq!(lists.hunks_in("a.txt", &other), ids(&["h1"]));
    }

    #[test]
    fn moving_hunks_to_a_nonexistent_list_or_an_unknown_hunk() {
        let mut lists = Changelists::default();
        let other = lists.create("Other").unwrap();
        lists.reconcile(&paths(&["a.txt"]));
        lists.reconcile_hunks("a.txt", &ids(&["h1"]));
        let before = lists.clone();

        assert_eq!(
            lists.move_hunks("a.txt", &ids(&["h1"]), "nothing"),
            Err(ChangelistError::NotFound)
        );
        assert_eq!(
            lists.move_hunks("a.txt", &ids(&["h1", "ghost"]), &other),
            Err(ChangelistError::HunkNotFound)
        );
        assert_eq!(lists, before);
    }

    #[test]
    fn moving_the_file_drags_its_hunks() {
        let mut lists = Changelists::default();
        let other = lists.create("Other").unwrap();
        let third = lists.create("Third").unwrap();
        lists.reconcile(&paths(&["a.txt", "b.txt"]));
        lists.reconcile_hunks("a.txt", &ids(&["h1", "h2"]));
        lists.reconcile_hunks("b.txt", &ids(&["k1"]));
        lists.move_hunks("a.txt", &ids(&["h2"]), &other).unwrap();

        lists.move_paths(&paths(&["a.txt"]), &third).unwrap();

        assert_eq!(lists.hunks_in("a.txt", &third), ids(&["h1", "h2"]));
        assert!(lists.hunks_in("a.txt", &other).is_empty());
        assert_eq!(lists.hunks_in("b.txt", &lists.active.clone()), ids(&["k1"]));
    }

    #[test]
    fn removing_the_list_sends_its_hunks_to_the_active_one() {
        let mut lists = Changelists::default();
        let other = lists.create("Other").unwrap();
        lists.reconcile(&paths(&["a.txt"]));
        lists.reconcile_hunks("a.txt", &ids(&["h1", "h2"]));
        lists.move_hunks("a.txt", &ids(&["h2"]), &other).unwrap();

        lists.remove(&other).unwrap();

        let active = lists.active.clone();
        assert_eq!(lists.hunks_in("a.txt", &active), ids(&["h1", "h2"]));
    }

    #[test]
    fn file_that_disappears_forgets_its_hunks() {
        let mut lists = Changelists::default();
        lists.reconcile(&paths(&["a.txt", "b.txt"]));
        lists.reconcile_hunks("a.txt", &ids(&["h1"]));
        lists.reconcile_hunks("b.txt", &ids(&["k1"]));

        assert!(lists.reconcile(&paths(&["b.txt"])));

        assert!(!lists.hunk_assignments.contains_key("a.txt"));
        assert!(lists.hunk_assignments.contains_key("b.txt"));
    }

    #[test]
    fn hunk_assigned_to_nonexistent_list_goes_back_to_the_files() {
        let mut lists = Changelists::default();
        lists.reconcile(&paths(&["a.txt"]));
        lists
            .hunk_assignments
            .entry("a.txt".to_string())
            .or_default()
            .insert("h1".to_string(), "deleted".to_string());

        assert!(lists.reconcile_hunks("a.txt", &ids(&["h1"])));

        assert_eq!(lists.hunks_in("a.txt", &lists.active.clone()), ids(&["h1"]));
    }

    #[test]
    fn file_without_hunks_or_assignments_leaves_no_trace() {
        let mut lists = Changelists::default();
        lists.reconcile(&paths(&["a.txt"]));
        let before = lists.clone();

        assert!(!lists.reconcile_hunks("a.txt", &[]));
        assert_eq!(lists, before);
    }

    #[test]
    fn saved_hunks_are_recovered_and_their_absence_is_tolerated() {
        let dir = TempDir::new().unwrap();
        let mut lists = Changelists::default();
        let other = lists.create("Other").unwrap();
        lists.reconcile(&paths(&["a.txt"]));
        lists.reconcile_hunks("a.txt", &ids(&["h1", "h2"]));
        lists.move_hunks("a.txt", &ids(&["h2"]), &other).unwrap();
        lists.save(dir.path()).unwrap();
        assert_eq!(Changelists::load(dir.path()), lists);

        let old = r#"{"lists": [{"id": "x", "name": "Mine"}], "active": "x", "assignments": {"a.txt": "x"}}"#;
        fs::write(dir.path().join(FILE_NAME), old).unwrap();
        let loaded = Changelists::load(dir.path());

        assert!(loaded.hunk_assignments.is_empty());
        assert_eq!(loaded.assignments["a.txt"], "x");
        assert!(!dir.path().join(CORRUPT_FILE_NAME).exists());
    }

    #[test]
    fn serializes_in_camel_case() {
        let value = serde_json::to_value(Changelists::default()).unwrap();
        assert!(value.get("lists").is_some());
        assert!(value.get("active").is_some());
        assert!(value.get("assignments").is_some());
        assert!(value.get("hunkAssignments").is_some());
    }

    fn restrict(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn missing_file_is_not_an_error() {
        let dir = TempDir::new().unwrap();
        assert_eq!(
            Changelists::try_load(dir.path()).unwrap(),
            Changelists::default()
        );
    }

    #[test]
    fn unreadable_file_is_an_error_and_is_not_touched() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join(FILE_NAME);
        fs::write(&file, br#"{"lists": [{"id": "x", "name": "Mine"}]}"#).unwrap();
        restrict(&file, 0o000);

        let result = Changelists::try_load(dir.path());

        restrict(&file, 0o644);
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert!(!dir.path().join(CORRUPT_FILE_NAME).exists());
        assert_eq!(
            fs::read(&file).unwrap(),
            br#"{"lists": [{"id": "x", "name": "Mine"}]}"#
        );
    }

    #[test]
    fn if_the_copy_cannot_be_kept_it_is_an_error() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join(FILE_NAME);
        fs::write(&file, b"{ broken").unwrap();
        restrict(dir.path(), 0o555);

        let result = Changelists::try_load(dir.path());

        restrict(dir.path(), 0o755);
        assert!(result.is_err());
        assert_eq!(fs::read(&file).unwrap(), b"{ broken");
        assert!(!dir.path().join(CORRUPT_FILE_NAME).exists());
    }

    #[test]
    fn a_second_damaged_file_keeps_the_first() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join(FILE_NAME);

        for contents in [&b"{ one"[..], b"{ two", b"{ three"] {
            fs::write(&file, contents).unwrap();
            assert_eq!(Changelists::load(dir.path()), Changelists::default());
        }

        let kept = |name: &str| fs::read(dir.path().join(name)).unwrap();
        assert_eq!(kept("changelists.json.corrupt"), b"{ one");
        assert_eq!(kept("changelists.json.corrupt.1"), b"{ two");
        assert_eq!(kept("changelists.json.corrupt.2"), b"{ three");
    }
}
