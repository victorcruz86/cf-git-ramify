use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImportedList {
    pub name: String,
    pub comment: String,
    pub active: bool,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImportedLists {
    pub lists: Vec<ImportedList>,
}

// IntelliJ writes the project paths as `$PROJECT_DIR$/path`.
const PROJECT_DIR: &str = "$PROJECT_DIR$/";
const WORKSPACE_FILE: &str = ".idea/workspace.xml";

// Path relative to the project, or `None` if it falls outside it or is not a file's.
fn project_path(raw: &str) -> Option<String> {
    let relative = raw.strip_prefix(PROJECT_DIR)?;
    let valid = relative
        .split('/')
        .all(|segment| !matches!(segment, "" | "." | ".."));
    valid.then(|| relative.to_string())
}

// `Boolean.parseBoolean`.
pub(crate) fn parse_bool(value: Option<impl AsRef<str>>) -> bool {
    value.is_some_and(|value| value.as_ref().eq_ignore_ascii_case("true"))
}

pub(crate) fn attributes(element: &BytesStart) -> Result<HashMap<String, String>, String> {
    let mut found = HashMap::new();
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| error.to_string())?;
        let value = attribute
            .normalized_value(XmlVersion::Implicit1_0)
            .map_err(|error| error.to_string())?;
        found.insert(attribute.key.as_ref().to_string(), value.into_owned());
    }
    Ok(found)
}

fn start_list(element: &BytesStart) -> Result<Option<ImportedList>, String> {
    let mut found = attributes(element)?;
    let name = found.remove("name").unwrap_or_default().trim().to_string();
    if name.is_empty() {
        return Ok(None);
    }
    Ok(Some(ImportedList {
        name,
        comment: found.remove("comment").unwrap_or_default(),
        active: found.get("default").is_some_and(|value| value == "true"),
        paths: Vec::new(),
    }))
}

// A change is located by `afterPath`, or by `beforePath` if it is a deletion. Folders do not count.
fn changed_path(element: &BytesStart) -> Result<Option<String>, String> {
    let found = attributes(element)?;
    let (path, dir) = match (found.get("afterPath"), found.get("beforePath")) {
        (Some(after), _) => (after, found.get("afterDir")),
        (None, Some(before)) => (before, found.get("beforeDir")),
        (None, None) => return Ok(None),
    };
    if dir.is_some_and(|value| value == "true") {
        return Ok(None);
    }
    Ok(project_path(path))
}

// Only reads the `ChangeListManager` component; the rest of the file is of no interest. A list
// without a name is ignored because there would be nothing to match it with.
pub fn read_workspace(xml: &str) -> Result<ImportedLists, String> {
    let mut reader = Reader::from_str(xml);
    let mut lists = Vec::new();
    let mut current: Option<ImportedList> = None;
    let mut in_component = false;
    let mut depth = 0usize;
    let mut seen_root = false;
    loop {
        let event = reader
            .read_event()
            .map_err(|error| format!("Malformed XML: {error}"))?;
        let (element, is_start) = match &event {
            Event::Start(element) => (Some(element), true),
            Event::Empty(element) => (Some(element), false),
            _ => (None, false),
        };
        if let Some(element) = element {
            seen_root = true;
            match element.name().as_ref() {
                "component" if is_start => {
                    in_component = attributes(element)?
                        .get("name")
                        .is_some_and(|name| name == "ChangeListManager");
                }
                "list" if in_component && current.is_none() => {
                    let list = start_list(element)?;
                    if is_start {
                        // A list without a name is still read so it can be closed where it belongs.
                        current = Some(list.unwrap_or_default());
                    } else {
                        lists.extend(list);
                    }
                }
                "change" => {
                    if let Some(list) = current.as_mut() {
                        list.paths.extend(changed_path(element)?);
                    }
                }
                _ => {}
            }
            if is_start {
                depth += 1;
            }
        }
        match event {
            Event::End(element) => {
                depth = depth.saturating_sub(1);
                match element.name().as_ref() {
                    "list" => {
                        if let Some(list) = current.take().filter(|list| !list.name.is_empty()) {
                            lists.push(list);
                        }
                    }
                    "component" => in_component = false,
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !seen_root {
        return Err("Malformed XML: the document is empty".to_string());
    }
    if depth != 0 {
        return Err("Malformed XML: the document is incomplete".to_string());
    }
    Ok(ImportedLists { lists })
}

// Opens `.idea/workspace.xml` read-only. Without that file there is nothing to import.
pub fn read_project(project: &Path) -> Result<ImportedLists, String> {
    match read_workspace_file(project)? {
        Some(xml) => read_workspace(&xml),
        None => Ok(ImportedLists::default()),
    }
}

fn read_workspace_file(project: &Path) -> Result<Option<String>, String> {
    let bytes = match fs::read(project.join(WORKSPACE_FILE)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Could not read {WORKSPACE_FILE}: {error}")),
    };
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| format!("{WORKSPACE_FILE} is not encoded in UTF-8"))
}

// `VcsConfiguration.USE_CUSTOM_SHELF_PATH` and `CUSTOM_SHELF_PATH`, options of the
// `VcsManagerConfiguration` component.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShelfSettings {
    pub use_custom: bool,
    pub custom_path: Option<String>,
}

const VCS_CONFIGURATION: &str = "VcsManagerConfiguration";
const DEFAULT_SHELF: &str = ".idea/shelf";
const PROJECT_DIR_MACRO: &str = "$PROJECT_DIR$";
const USER_HOME_MACRO: &str = "$USER_HOME$";

pub fn read_shelf_settings(xml: &str) -> Result<ShelfSettings, String> {
    let mut reader = Reader::from_str(xml);
    let mut settings = ShelfSettings::default();
    let mut in_component = false;
    loop {
        let event = reader
            .read_event()
            .map_err(|error| format!("Malformed XML: {error}"))?;
        match &event {
            Event::Start(element) if element.name().as_ref() == "component" => {
                in_component = attributes(element)?
                    .get("name")
                    .is_some_and(|name| name == VCS_CONFIGURATION);
            }
            Event::Start(element) | Event::Empty(element)
                if in_component && element.name().as_ref() == "option" =>
            {
                let mut found = attributes(element)?;
                let value = found.remove("value");
                match found.get("name").map(String::as_str) {
                    Some("USE_CUSTOM_SHELF_PATH") => settings.use_custom = parse_bool(value),
                    Some("CUSTOM_SHELF_PATH") => settings.custom_path = value,
                    _ => {}
                }
            }
            Event::End(element) if element.name().as_ref() == "component" => in_component = false,
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(settings)
}

// Where IntelliJ keeps the project's shelf (`ShelveChangesManager.getShelfPath`): the custom path
// when it is on and set, otherwise `.idea/shelf`. The custom path is stored with the project's
// macros collapsed; any other macro, or a path that is still relative, is refused.
pub fn shelf_folder(project: &Path) -> Result<PathBuf, String> {
    let settings = match read_workspace_file(project)? {
        Some(xml) => read_shelf_settings(&xml)?,
        None => ShelfSettings::default(),
    };
    let custom = settings
        .custom_path
        .filter(|path| settings.use_custom && !path.is_empty());
    let Some(custom) = custom else {
        return Ok(project.join(DEFAULT_SHELF));
    };
    let project_dir = project
        .to_str()
        .ok_or_else(|| "The project path is not UTF-8".to_string())?;
    let mut expanded = custom.replace(PROJECT_DIR_MACRO, project_dir);
    if expanded.contains(USER_HOME_MACRO) {
        let home = std::env::var("HOME").map_err(|_| "HOME is not set".to_string())?;
        expanded = expanded.replace(USER_HOME_MACRO, &home);
    }
    let path = PathBuf::from(&expanded);
    let macro_left = regex::Regex::new(r"\$[A-Za-z_][A-Za-z0-9_]*\$")
        .is_ok_and(|pattern| pattern.is_match(&expanded));
    if macro_left || !path.is_absolute() {
        return Err(format!(
            "The custom shelf path \"{custom}\" cannot be resolved"
        ));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changelists::Changelists;
    use crate::commands;
    use crate::test_support::TempRepo;
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;
    use std::fs;
    use tempfile::TempDir;

    const FIXTURE: &str = include_str!("../testdata/workspace.xml");

    fn list<'a>(imported: &'a ImportedLists, name: &str) -> &'a ImportedList {
        imported.lists.iter().find(|l| l.name == name).unwrap()
    }

    fn paths(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn workspace(body: &str) -> String {
        format!(
            r#"<project version="4"><component name="ChangeListManager">{body}</component></project>"#
        )
    }

    #[test]
    fn reads_lists_and_paths() {
        let imported = read_workspace(FIXTURE).unwrap();

        let names: Vec<_> = imported.lists.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["Changes", "Refactor & cleanup", "Empty"]);
        let changes = list(&imported, "Changes");
        assert_eq!(changes.comment, "Work in progress");
        assert_eq!(changes.paths[0], "src/a.txt");
    }

    #[test]
    fn decodes_escapes_spaces_and_accents() {
        let imported = read_workspace(FIXTURE).unwrap();

        assert_eq!(
            list(&imported, "Changes").paths,
            paths(&[
                "src/a.txt",
                "docs/Café menu.md",
                "src/new.txt",
                "src/Tom & Jerry.txt",
            ])
        );
        assert_eq!(
            list(&imported, "Refactor & cleanup").comment,
            "Mind the \"quotes\""
        );
    }

    #[test]
    fn new_uses_after_and_deleted_uses_before() {
        let imported = read_workspace(&workspace(
            r#"<list name="L" comment="">
                 <change afterPath="$PROJECT_DIR$/new.txt" afterDir="false" />
                 <change beforePath="$PROJECT_DIR$/deleted.txt" beforeDir="false" />
               </list>"#,
        ))
        .unwrap();

        assert_eq!(
            list(&imported, "L").paths,
            paths(&["new.txt", "deleted.txt"])
        );
    }

    #[test]
    fn ignores_folders() {
        let imported = read_workspace(&workspace(
            r#"<list name="L" comment="">
                 <change afterPath="$PROJECT_DIR$/dir" afterDir="true" />
                 <change beforePath="$PROJECT_DIR$/other" beforeDir="true" />
                 <change afterPath="$PROJECT_DIR$/f.txt" afterDir="false" />
               </list>"#,
        ))
        .unwrap();

        assert_eq!(list(&imported, "L").paths, paths(&["f.txt"]));
    }

    #[test]
    fn ignores_paths_outside_the_project() {
        let imported = read_workspace(FIXTURE).unwrap();

        assert_eq!(
            list(&imported, "Refactor & cleanup").paths,
            paths(&["src/old.txt"])
        );
    }

    #[test]
    fn ignores_root_paths_or_ones_with_odd_segments() {
        let imported = read_workspace(&workspace(
            r#"<list name="L" comment="">
                 <change afterPath="$PROJECT_DIR$" afterDir="false" />
                 <change afterPath="$PROJECT_DIR$/" afterDir="false" />
                 <change afterPath="$PROJECT_DIR$/a/../b.txt" afterDir="false" />
                 <change afterPath="$PROJECT_DIR$/./b.txt" afterDir="false" />
                 <change afterPath="$PROJECT_DIR$/a//b.txt" afterDir="false" />
                 <change afterPath="$PROJECT_DIR$X/b.txt" afterDir="false" />
               </list>"#,
        ))
        .unwrap();

        assert!(list(&imported, "L").paths.is_empty());
    }

    #[test]
    fn marks_the_default_list() {
        let imported = read_workspace(FIXTURE).unwrap();

        assert!(list(&imported, "Changes").active);
        assert!(!list(&imported, "Refactor & cleanup").active);
        assert!(!list(&imported, "Empty").active);
    }

    #[test]
    fn without_component() {
        let xml = r#"<project version="4"><component name="Other"><list name="X" comment=""><change afterPath="$PROJECT_DIR$/a" afterDir="false" /></list></component></project>"#;

        assert_eq!(read_workspace(xml).unwrap(), ImportedLists::default());
        assert_eq!(
            read_workspace(r#"<project version="4"/>"#).unwrap(),
            ImportedLists::default()
        );
    }

    #[test]
    fn nameless_list_is_ignored() {
        let imported = read_workspace(&workspace(
            r#"<list comment=""><change afterPath="$PROJECT_DIR$/a" afterDir="false" /></list>
               <list name="  " comment="" />"#,
        ))
        .unwrap();

        assert!(imported.lists.is_empty());
    }

    #[test]
    fn broken_xml_gives_an_error() {
        assert!(read_workspace("this is not xml").is_err());
        assert!(read_workspace("").is_err());
        assert!(
            read_workspace(r#"<project><component name="ChangeListManager"></project>"#).is_err()
        );
        let truncated = &FIXTURE[..FIXTURE.len() / 2];
        assert!(read_workspace(truncated).is_err());
        assert!(read_workspace(&workspace(r#"<list name="a&nope;" comment="" />"#)).is_err());
    }

    fn merged(imported: &str, start: Changelists) -> Changelists {
        let mut lists = start;
        lists.merge_import(read_workspace(imported).unwrap());
        lists
    }

    fn id_of(lists: &Changelists, name: &str) -> String {
        lists
            .lists
            .iter()
            .find(|l| l.name == name)
            .unwrap()
            .id
            .clone()
    }

    #[test]
    fn import_twice() {
        let once = merged(FIXTURE, Changelists::default());
        let twice = merged(FIXTURE, once.clone());

        assert_eq!(twice, once);
        let names: Vec<_> = twice.lists.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["Changes", "Refactor & cleanup", "Empty"]);
    }

    #[test]
    fn the_existing_list_keeps_id_and_comment() {
        let mut start = Changelists::default();
        start.lists[0].comment = "Mine".to_string();
        let id = start.lists[0].id.clone();

        let lists = merged(FIXTURE, start);

        assert_eq!(id_of(&lists, "Changes"), id);
        assert_eq!(lists.lists[0].comment, "Mine");
        let new = lists.lists.iter().find(|l| l.name == "Empty").unwrap();
        assert_eq!(new.comment, "");
        assert_eq!(
            lists
                .lists
                .iter()
                .find(|l| l.name == "Refactor & cleanup")
                .unwrap()
                .comment,
            "Mind the \"quotes\""
        );
    }

    #[test]
    fn paths_move_to_the_intellij_list() {
        let mut start = Changelists::default();
        let own = start.create("My list").unwrap();
        start.reconcile(&paths(&["src/a.txt", "src/old.txt", "other.txt"]));
        start.move_paths(&paths(&["src/a.txt"]), &own).unwrap();

        let lists = merged(FIXTURE, start);

        assert_eq!(lists.assignments["src/a.txt"], id_of(&lists, "Changes"));
        assert_eq!(
            lists.assignments["src/old.txt"],
            id_of(&lists, "Refactor & cleanup")
        );
        assert_eq!(lists.assignments["other.txt"], lists.active);
    }

    #[test]
    fn takes_the_intellij_active_one_if_its_own_has_no_files() {
        let mut start = Changelists::default();
        let own = start.create("My list").unwrap();
        start.set_active(&own).unwrap();
        let xml = workspace(
            r#"<list default="true" name="Changes" comment=""><change afterPath="$PROJECT_DIR$/a" afterDir="false" /></list>
               <list name="My list" comment="" />"#,
        );

        let lists = merged(&xml, start);

        assert_eq!(lists.active, id_of(&lists, "Changes"));
    }

    #[test]
    fn keeps_its_own_active_one_if_it_already_has_files() {
        let mut start = Changelists::default();
        let own = start.create("My list").unwrap();
        start.set_active(&own).unwrap();
        start.reconcile(&paths(&["x.txt"]));

        let lists = merged(FIXTURE, start);

        assert_eq!(lists.active, own);
    }

    #[test]
    fn without_an_active_list_in_intellij_the_active_one_does_not_change() {
        let start = Changelists::default();
        let active = start.active.clone();

        let lists = merged(&workspace(r#"<list name="Other" comment="" />"#), start);

        assert_eq!(lists.active, active);
    }

    #[test]
    fn repeated_names_in_the_xml_do_not_duplicate_lists() {
        let xml = workspace(
            r#"<list name="L" comment="one"><change afterPath="$PROJECT_DIR$/a" afterDir="false" /></list>
               <list name="L" comment="two"><change afterPath="$PROJECT_DIR$/b" afterDir="false" /></list>"#,
        );

        let lists = merged(&xml, Changelists::default());

        assert_eq!(lists.lists.iter().filter(|l| l.name == "L").count(), 1);
        assert_eq!(lists.assignments["a"], id_of(&lists, "L"));
        assert_eq!(lists.assignments["b"], id_of(&lists, "L"));
    }

    fn snapshot(dir: &Path) -> BTreeMap<String, String> {
        fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let name = path.strip_prefix(base).unwrap().display().to_string();
                if path.is_dir() {
                    out.insert(name, "dir".to_string());
                    walk(base, &path, out);
                } else {
                    let digest = Sha256::digest(fs::read(&path).unwrap());
                    out.insert(name, digest.iter().map(|b| format!("{b:02x}")).collect());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(dir, dir, &mut out);
        out
    }

    #[test]
    fn does_not_write_to_idea() {
        let repo = TempRepo::new();
        let idea = repo.path().join(".idea");
        fs::create_dir_all(idea.join("shelf")).unwrap();
        fs::write(idea.join("workspace.xml"), FIXTURE).unwrap();
        fs::write(idea.join("misc.xml"), "<project/>").unwrap();
        let data = TempDir::new().unwrap();
        let before = snapshot(&idea);
        assert_eq!(before.len(), 3);

        let first = commands::import_changelists(repo.path(), data.path()).unwrap();
        let second = commands::import_changelists(repo.path(), data.path()).unwrap();

        assert_eq!(snapshot(&idea), before);
        assert_eq!(first, second);
        assert_eq!(second.lists.len(), 3);
    }

    fn project_with_workspace(xml: Option<&str>) -> TempDir {
        let dir = TempDir::new().unwrap();
        if let Some(xml) = xml {
            fs::create_dir(dir.path().join(".idea")).unwrap();
            fs::write(dir.path().join(WORKSPACE_FILE), xml).unwrap();
        }
        dir
    }

    fn shelf_settings(use_custom: &str, custom: &str) -> String {
        format!(
            r#"<project version="4"><component name="VcsManagerConfiguration">
<option name="USE_CUSTOM_SHELF_PATH" value="{use_custom}" />
<option name="CUSTOM_SHELF_PATH" value="{custom}" />
</component></project>"#
        )
    }

    #[test]
    fn shelf_folder_follows_the_custom_shelf_options() {
        let default = |dir: &TempDir| dir.path().join(".idea/shelf");
        // No `workspace.xml`, or one without the component: the default place.
        let missing = project_with_workspace(None);
        assert_eq!(shelf_folder(missing.path()).unwrap(), default(&missing));
        let other = project_with_workspace(Some(&workspace("")));
        assert_eq!(shelf_folder(other.path()).unwrap(), default(&other));
        // On but empty, or set but off: the default place (`USE_CUSTOM_SHELF_PATH ? … : null`).
        let empty = project_with_workspace(Some(&shelf_settings("true", "")));
        assert_eq!(shelf_folder(empty.path()).unwrap(), default(&empty));
        let off = project_with_workspace(Some(&shelf_settings("false", "/elsewhere")));
        assert_eq!(shelf_folder(off.path()).unwrap(), default(&off));
        // `Boolean.parseBoolean` ignores case.
        let upper = project_with_workspace(Some(&shelf_settings("TRUE", "/elsewhere")));
        assert_eq!(
            shelf_folder(upper.path()).unwrap(),
            PathBuf::from("/elsewhere")
        );
        assert_eq!(
            read_shelf_settings(&shelf_settings("TRUE", "/elsewhere")).unwrap(),
            ShelfSettings {
                use_custom: true,
                custom_path: Some("/elsewhere".to_string()),
            }
        );

        // The project's macros are expanded.
        let home = std::env::var("HOME").unwrap();
        let in_home = project_with_workspace(Some(&shelf_settings("true", "$USER_HOME$/shelves")));
        assert_eq!(
            shelf_folder(in_home.path()).unwrap(),
            Path::new(&home).join("shelves")
        );
        let beside = project_with_workspace(Some(&shelf_settings("true", "$PROJECT_DIR$/../s")));
        assert_eq!(
            shelf_folder(beside.path()).unwrap(),
            PathBuf::from(format!("{}/../s", beside.path().display()))
        );
        // A `$` that is not a macro is a plain character.
        let dollar = project_with_workspace(Some(&shelf_settings("true", "/a$b/shelf")));
        assert_eq!(
            shelf_folder(dollar.path()).unwrap(),
            PathBuf::from("/a$b/shelf")
        );

        // Another macro, or a path that stays relative, cannot be resolved.
        for custom in ["$APPLICATION_HOME_DIR$/shelf", "../shelf", "shelf"] {
            let refused = project_with_workspace(Some(&shelf_settings("true", custom)));
            assert!(shelf_folder(refused.path()).is_err(), "{custom}");
        }
        let broken = project_with_workspace(Some("<project"));
        assert!(shelf_folder(broken.path()).is_err());
    }

    #[test]
    fn project_without_idea_gives_an_empty_import() {
        let dir = TempDir::new().unwrap();
        assert_eq!(read_project(dir.path()).unwrap(), ImportedLists::default());

        fs::create_dir(dir.path().join(".idea")).unwrap();
        assert_eq!(read_project(dir.path()).unwrap(), ImportedLists::default());
    }

    #[test]
    fn reads_the_project_file() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join(".idea")).unwrap();
        fs::write(dir.path().join(".idea/workspace.xml"), FIXTURE).unwrap();

        assert_eq!(
            read_project(dir.path()).unwrap(),
            read_workspace(FIXTURE).unwrap()
        );
    }

    #[test]
    fn unreadable_or_broken_file_gives_an_error() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join(".idea")).unwrap();
        let file = dir.path().join(".idea/workspace.xml");
        fs::write(&file, "<project><broken").unwrap();
        assert!(read_project(dir.path()).is_err());

        fs::write(&file, FIXTURE).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
        let result = read_project(dir.path());
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(result.is_err());

        fs::write(&file, [0xff, 0xfe, 0x00]).unwrap();
        assert!(read_project(dir.path()).is_err());
    }
}
