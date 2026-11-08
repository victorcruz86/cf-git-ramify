// The shelf: `ShelveChangesManager`'s store in IntelliJ's layout, under the repository's data
// folder (`<data>/shelf/`), never in `.idea/`. Each list has a folder with its `shelved.patch` and
// the whole copies of its binary files, and a `<name>.xml` beside the folder. `.idea/shelf` is only
// read, by `import`.
//
// What comes from XML is untrusted. A name reaches a path only after `valid_name`, and the store
// names its lists itself (`unique_name`), never after a name read from XML. A list is read only if
// its folder `<root>/<name>` is a real folder (not a link), its patch is exactly
// `<root>/<name>/shelved.patch`, and each binary copy is inside that folder.
pub mod shelve;
pub mod tab;
pub mod unshelve;

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use quick_xml::events::attributes::Attribute;
use quick_xml::events::{BytesEnd, BytesStart, Event};
use quick_xml::name::QName;
use quick_xml::{Reader, Writer};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::commands::ApiError;
use crate::git;
use crate::idea_import;
use crate::patch::{self, FilePatch, Patch, PatchError, WriteOptions};
use crate::snapshots;
use crate::storage;

const SHELF_DIR: &str = "shelf";
// `DEFAULT_PATCH_NAME` + `.` + `VcsConfiguration.PATCH`.
const PATCH_FILE: &str = "shelved.patch";
// rama's own, beside the copies: the blob HEAD had for the before path of each binary file when it
// was shelved, so that an unshelve can tell that HEAD changed that file meanwhile. IntelliJ keeps
// nothing of the kind; a list without it (an imported one) is put back without that check.
const BASE_IDS_FILE: &str = ".base-ids.json";
const XML_EXTENSION: &str = "xml";
// The IntelliJ lists already imported, so that one deleted here does not come back.
const IMPORTED_FILE: &str = "imported.json";
const IMPORTED_CORRUPT: &str = "imported.json.corrupt";
// `PatchNameChecker.MAX`, and the cut of `shortenAndSanitize` (`MAX - 10`), both in UTF-16 units.
const NAME_MAX: usize = 100;
const NAME_CUT: usize = NAME_MAX - 10;
// rama's own bound, in UTF-8 bytes: a file name holds 255 of them on APFS, and the XML file adds
// `.xml`. The prefix keeps room for the number of `findSequentFile` (20 digits at most).
const NAME_BYTES: usize = 240;
const PREFIX_BYTES: usize = NAME_BYTES - 20;
const UNNAMED: &str = "unnamed";
// A file name on APFS: 255 bytes.
const FILE_NAME_BYTES: usize = 255;
const DAY_MS: u64 = 24 * 60 * 60 * 1000;
// `cleanDeletedOlderOneWeek`.
const DELETED_KEPT_MS: u64 = 7 * DAY_MS;
// rama's own: where what the store erases is kept (`erase`), beside `shelf/`. The list's name is
// cut in its folder's name so that the moment and a number fit in 255 bytes.
const ERASED_DIR: &str = "shelf-erased";
const ERASED_NAME_BYTES: usize = 200;
const PROJECT_DIR_PREFIX: &str = "$PROJECT_DIR$/";

// `ShelvedChangeList`, `ShelvedBinaryFile` and the old `ShelveChangesManager` component.
const CHANGELIST: &str = "changelist";
const RECYCLED_CHANGELIST: &str = "recycled_changelist";
const COMPONENT: &str = "component";
const SHELVE_COMPONENT: &str = "ShelveChangesManager";
const BINARY: &str = "binary";
const OPTION: &str = "option";
const PATH_OPTION: &str = "PATH";
const DESCRIPTION_OPTION: &str = "DESCRIPTION";
const BEFORE_OPTION: &str = "BEFORE_PATH";
const AFTER_OPTION: &str = "AFTER_PATH";
const SHELVED_OPTION: &str = "SHELVED_PATH";

// rama's own: a list asked for by name that is not in the store (any more).
fn list_gone(name: &str) -> String {
    format!("\"{name}\" is no longer in the shelf")
}

// Every change to the store holds this lock: the daily cleanup runs beside the commands.
static STORE: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    STORE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// `SHELF_TOPIC`: each store's generation, moved by every write to it (`store`, `erase`), so that
// `repo-changed` can say that the shelf changed and the Shelf tab reads it only then.
static GENERATIONS: Mutex<BTreeMap<PathBuf, u64>> = Mutex::new(BTreeMap::new());

fn changed(root: &Path) {
    let mut generations = GENERATIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *generations.entry(root.to_path_buf()).or_default() += 1;
}

// How many times the store of `data` was written since the app started.
pub fn generation(data: &Path) -> u64 {
    let generations = GENERATIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    generations.get(&shelf_root(data)).copied().unwrap_or(0)
}

// `ShelvedBinaryFile`. `before: None` is an added file and `shelved: None` a deleted one; the paths
// are relative to the repository, the copy is a file in the list's folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShelvedBinary {
    pub before: Option<String>,
    pub after: Option<String>,
    pub shelved: Option<PathBuf>,
}

// `ShelvedChangeList`: `name` is the scheme and folder name, `description` what the user sees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShelvedList {
    pub name: String,
    pub description: String,
    pub date_ms: u64,
    pub recycled: bool,
    pub to_delete: bool,
    pub deleted: bool,
    pub patch: PathBuf,
    pub binaries: Vec<ShelvedBinary>,
}

// A binary file to shelve, by its paths in the repository. The after file is copied as it is on
// disk (`shelveBinaryFile`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinaryChange {
    pub before: Option<String>,
    pub after: Option<String>,
}

// `skipped` counts the lists already imported and those that could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub added: usize,
    pub skipped: usize,
}

// One list as its XML says it, before any check.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct XmlBinary {
    before: Option<String>,
    after: Option<String>,
    shelved: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct XmlList {
    name: Option<String>,
    date: u64,
    recycled: bool,
    to_delete: bool,
    deleted: bool,
    path: Option<String>,
    description: Option<String>,
    binaries: Vec<XmlBinary>,
}

// What the lists of the import are recognised by.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportKey {
    description: String,
    date: u64,
    sha256: String,
    // The paths of the binary files and the SHA-256 of their copies. A record of an earlier build
    // has none, and then matches whatever copies a list has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    binaries: Option<String>,
}

impl ImportKey {
    fn same(&self, other: &ImportKey) -> bool {
        self.description == other.description
            && self.date == other.date
            && self.sha256 == other.sha256
            && match (&self.binaries, &other.binaries) {
                (Some(mine), Some(theirs)) => mine == theirs,
                _ => true,
            }
    }
}

// The binary files of a list for its `ImportKey`: before and after paths and the copy's hash, in
// the list's order. A copy that cannot be read counts as none.
fn binaries_key<'a>(
    binaries: impl IntoIterator<Item = (&'a Option<String>, &'a Option<String>, Option<&'a Path>)>,
) -> String {
    let mut text = String::new();
    for (before, after, copy) in binaries {
        let content = copy.and_then(|copy| fs::read(copy).ok());
        text.push_str(&format!(
            "{:?}\0{:?}\0{}\n",
            before,
            after,
            content.as_deref().map(sha256).unwrap_or_default()
        ));
    }
    sha256(text.as_bytes())
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Imported {
    lists: Vec<ImportKey>,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

pub(crate) fn millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

// `readExternal` of `ShelvedChangeList` and `ShelvedBinaryFile` (`DefaultJDOMExternalizer`): the
// last option of a name wins; an empty `PATH` is no path. The scheme files have one `changelist`
// at the root; the old `ShelveChangesManager` component of `workspace.xml` has `changelist` and
// `recycled_changelist` children, and there the element says whether the list is recycled
// (`migrateOldShelfInfo`). A list without a valid `date` is left out: `readExternal` fails on it.
fn read_xml(bytes: &[u8]) -> Result<Vec<XmlList>, String> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| "The file is not encoded in UTF-8".to_string())?;
    let mut reader = Reader::from_str(text);
    let mut lists = Vec::new();
    let mut depth = 0usize;
    let mut component: Option<usize> = None;
    // The depth of the list's element, and the list unless its date was not valid.
    let mut current: Option<(usize, Option<XmlList>)> = None;
    let mut binary: Option<(usize, XmlBinary)> = None;
    let mut seen_root = false;
    loop {
        let event = reader
            .read_event()
            .map_err(|error| format!("Malformed XML: {error}"))?;
        match &event {
            Event::Start(element) | Event::Empty(element) => {
                seen_root = true;
                let empty = matches!(event, Event::Empty(_));
                let name = element.name();
                let name = name.as_ref();
                let in_component = component.is_some_and(|level| level + 1 == depth);
                let list_level = current.as_ref().map(|(level, _)| *level);
                if name == COMPONENT && depth == 0 {
                    let found = idea_import::attributes(element)?;
                    if found
                        .get("name")
                        .is_some_and(|name| name == SHELVE_COMPONENT)
                    {
                        component = Some(depth);
                    }
                } else if current.is_none()
                    && ((name == CHANGELIST && depth == 0)
                        || (in_component && (name == CHANGELIST || name == RECYCLED_CHANGELIST)))
                {
                    let mut list = start_list(element)?;
                    if in_component {
                        if let Some(list) = list.as_mut() {
                            list.recycled = name == RECYCLED_CHANGELIST;
                        }
                    }
                    if empty {
                        lists.extend(list);
                    } else {
                        current = Some((depth, list));
                    }
                } else if name == OPTION
                    && binary.as_ref().is_some_and(|(level, _)| level + 1 == depth)
                {
                    let (key, value) = option(element)?;
                    if let Some((_, binary)) = binary.as_mut() {
                        match key.as_deref() {
                            Some(BEFORE_OPTION) => binary.before = value,
                            Some(AFTER_OPTION) => binary.after = value,
                            Some(SHELVED_OPTION) => binary.shelved = value,
                            _ => {}
                        }
                    }
                } else if list_level.is_some_and(|level| level + 1 == depth) {
                    if let Some((_, Some(list))) = current.as_mut() {
                        if name == OPTION {
                            let (key, value) = option(element)?;
                            match key.as_deref() {
                                Some(PATH_OPTION) => {
                                    list.path = value.filter(|value| !value.is_empty())
                                }
                                Some(DESCRIPTION_OPTION) => list.description = value,
                                _ => {}
                            }
                        } else if name == BINARY && binary.is_none() {
                            if empty {
                                list.binaries.push(XmlBinary::default());
                            } else {
                                binary = Some((depth, XmlBinary::default()));
                            }
                        }
                    }
                }
                if !empty {
                    depth += 1;
                }
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if binary.as_ref().is_some_and(|(level, _)| *level == depth) {
                    let (_, done) = binary.take().unwrap_or_default();
                    if let Some((_, Some(list))) = current.as_mut() {
                        list.binaries.push(done);
                    }
                }
                if current.as_ref().is_some_and(|(level, _)| *level == depth) {
                    if let Some((_, Some(list))) = current.take() {
                        lists.push(list);
                    }
                }
                if component == Some(depth) {
                    component = None;
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !seen_root || depth != 0 {
        return Err("Malformed XML: the document is empty or incomplete".to_string());
    }
    Ok(lists)
}

fn start_list(element: &BytesStart) -> Result<Option<XmlList>, String> {
    let found = idea_import::attributes(element)?;
    let Some(date) = found.get("date").and_then(|date| date.parse::<u64>().ok()) else {
        return Ok(None);
    };
    Ok(Some(XmlList {
        name: found.get("name").cloned(),
        date,
        recycled: idea_import::parse_bool(found.get("recycled")),
        to_delete: idea_import::parse_bool(found.get("toDelete")),
        deleted: idea_import::parse_bool(found.get("deleted")),
        ..XmlList::default()
    }))
}

fn option(element: &BytesStart) -> Result<(Option<String>, Option<String>), String> {
    let mut found = idea_import::attributes(element)?;
    Ok((found.remove("name"), found.remove("value")))
}

// `JDOMUtil.escapeText(value, false, true)`, the attribute escaping of `JbXmlOutputter`. A
// character that XML 1.0 cannot hold is dropped: the file would no longer be read.
fn escape_attribute(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\n' => escaped.push_str("&#10;"),
            '\r' => escaped.push_str("&#13;"),
            '\t' => escaped.push_str("&#9;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '&' => escaped.push_str("&amp;"),
            '\u{0}'..='\u{1f}' | '\u{fffe}' | '\u{ffff}' => {}
            c => escaped.push(c),
        }
    }
    escaped
}

fn push(element: &mut BytesStart, key: &'static str, value: &str) {
    element.push_attribute(Attribute {
        key: QName(key),
        value: Cow::Owned(escape_attribute(value)),
    });
}

fn write_option(writer: &mut Writer<Vec<u8>>, name: &str, value: Option<&str>) {
    let mut element = BytesStart::new(OPTION);
    push(&mut element, "name", name);
    if let Some(value) = value {
        push(&mut element, "value", value);
    }
    write(writer, Event::Empty(element));
}

fn write(writer: &mut Writer<Vec<u8>>, event: Event) {
    // Writing to memory does not fail.
    let _ = writer.write_event(event);
}

// `ShelvedChangeList.writeExternal` as `JbXmlOutputter` prints it: `PATH` first, then the fields
// (`DESCRIPTION`), then the attributes `name`, `date`, `recycled` and, only when true, `toDelete`
// and `deleted`; two spaces of indent, ` />`, `\n`, and no newline at the end.
fn write_xml(list: &XmlList) -> Vec<u8> {
    let mut writer = Writer::new_with_indent(Vec::new(), b' ', 2);
    writer.config_mut().add_space_before_slash_in_empty_elements = true;
    let mut start = BytesStart::new(CHANGELIST);
    if let Some(name) = &list.name {
        push(&mut start, "name", name);
    }
    push(&mut start, "date", &list.date.to_string());
    push(
        &mut start,
        "recycled",
        if list.recycled { "true" } else { "false" },
    );
    if list.to_delete {
        push(&mut start, "toDelete", "true");
    }
    if list.deleted {
        push(&mut start, "deleted", "true");
    }
    write(&mut writer, Event::Start(start));
    if let Some(path) = &list.path {
        write_option(&mut writer, PATH_OPTION, Some(path));
    }
    write_option(&mut writer, DESCRIPTION_OPTION, list.description.as_deref());
    for binary in &list.binaries {
        write(&mut writer, Event::Start(BytesStart::new(BINARY)));
        write_option(&mut writer, BEFORE_OPTION, binary.before.as_deref());
        write_option(&mut writer, AFTER_OPTION, binary.after.as_deref());
        write_option(&mut writer, SHELVED_OPTION, binary.shelved.as_deref());
        write(&mut writer, Event::End(BytesEnd::new(BINARY)));
    }
    write(&mut writer, Event::End(BytesEnd::new(CHANGELIST)));
    writer.into_inner()
}

// `Character.isWhitespace`: the Unicode space separators but the no-break ones, and the controls
// TAB to CR and FS to US.
fn java_whitespace(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n'
            | '\u{b}'
            | '\u{c}'
            | '\r'
            | '\u{1c}'..='\u{1f}'
            | ' '
            | '\u{1680}'
            | '\u{2000}'..='\u{2006}'
            | '\u{2008}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{205f}'
            | '\u{3000}'
    )
}

// `PathUtilRt.suggestFileName(text, false, false)`: separators, the characters Windows forbids,
// `;`, controls, `.` and whitespace become `_`.
fn suggest_file_name(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '/' | '\\' | '<' | '>' | ':' | '"' | '|' | '?' | '*' | ';' | '.' => '_',
            c if (c as u32) < 32 || java_whitespace(c) => '_',
            c => c,
        })
        .collect()
}

fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

// `shortenAndSanitize`: `unnamed` when empty, cut at 90 UTF-16 units. A pair that the cut would
// split is left out whole, and the cut also stops at `PREFIX_BYTES` UTF-8 bytes, on a character
// boundary.
fn shorten_and_sanitize(description: &str) -> String {
    let sanitized = suggest_file_name(description);
    if sanitized.is_empty() {
        return UNNAMED.to_string();
    }
    let mut cut = String::new();
    let mut units = 0;
    for c in sanitized.chars() {
        units += c.len_utf16();
        if units > NAME_CUT || cut.len() + c.len_utf8() > PREFIX_BYTES {
            break;
        }
        cut.push(c);
    }
    cut
}

// `FileUtil.findSequentFile`: `prefix.ext`, `prefix1.ext`, `prefix2.ext`, …
fn sequent(prefix: &str, extension: &str, taken: impl Fn(&str) -> bool) -> String {
    let extension = if extension.is_empty() {
        String::new()
    } else {
        format!(".{extension}")
    };
    let mut candidate = format!("{prefix}{extension}");
    let mut postfix = 0u64;
    while taken(&candidate) {
        postfix += 1;
        candidate = format!("{prefix}{postfix}{extension}");
    }
    candidate
}

fn exists(path: &Path) -> bool {
    path.symlink_metadata().is_ok()
}

// `suggestPatchName(…, extension "")`: a name of fewer than 100 units is taken; otherwise the
// prefix loses a character and the search starts again. A name is free only when neither the
// folder nor the XML file exists.
fn unique_name(root: &Path, description: &str) -> String {
    let mut prefix = shorten_and_sanitize(description);
    loop {
        let name = sequent(&prefix, "", |candidate| {
            exists(&root.join(candidate)) || exists(&xml_file(root, candidate))
        });
        let too_long = utf16_len(&name) >= NAME_MAX || name.len() > NAME_BYTES;
        if too_long && prefix.pop().is_some() {
            continue;
        }
        return name;
    }
}

// `FileUtilRt.getNameWithoutExtension` and `getExtension`: around the last `.`.
fn split_extension(file_name: &str) -> (&str, &str) {
    file_name.rsplit_once('.').unwrap_or((file_name, ""))
}

// A copy named after `file_name` that does not exist yet in `folder` (`c.png`, `c1.png`, …). It is
// created with `create_new`, so nothing is ever overwritten.
fn copy_into(folder: &Path, file_name: &str, source: &Path) -> io::Result<PathBuf> {
    let (stem, extension) = split_extension(file_name);
    // `findSequentFile`, with the stem cut when the number would make the name longer than a file
    // name can be; an extension too long for that goes with the stem.
    let (stem, extension) = match extension.len() + 1 + 20 < FILE_NAME_BYTES {
        true if !extension.is_empty() => (stem, format!(".{extension}")),
        _ => (file_name, String::new()),
    };
    let mut postfix = 0u64;
    let name = loop {
        let number = match postfix {
            0 => String::new(),
            _ => postfix.to_string(),
        };
        let mut cut = stem
            .len()
            .min(FILE_NAME_BYTES.saturating_sub(number.len() + extension.len()));
        while !stem.is_char_boundary(cut) {
            cut -= 1;
        }
        let candidate = format!("{}{number}{extension}", &stem[..cut]);
        if !exists(&folder.join(&candidate)) {
            break candidate;
        }
        postfix += 1;
    };
    let target = folder.join(name);
    let mut from = fs::File::open(source)?;
    let mut to = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)?;
    io::copy(&mut from, &mut to)?;
    to.sync_all()?;
    Ok(target)
}

fn file_name_of(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn regular_file(path: &Path) -> bool {
    path.symlink_metadata().is_ok_and(|meta| meta.is_file())
}

// A scheme name that is a single plain file name: no separator, not hidden, not `..`.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.contains(['/', '\\', '\0'])
        && name.len() <= NAME_BYTES
}

// A path relative to the repository: no root, no `..`, no empty part.
fn valid_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\0')
        && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

// An absolute path written as it is (only `Normal` parts after the root) that is inside `folder`.
fn inside(folder: &Path, value: &str) -> Option<PathBuf> {
    let path = Path::new(value);
    let plain = path
        .components()
        .skip(1)
        .all(|part| matches!(part, Component::Normal(_)));
    (path.is_absolute() && plain && path.starts_with(folder) && path != folder)
        .then(|| path.to_path_buf())
}

fn shelf_root(data: &Path) -> PathBuf {
    data.join(SHELF_DIR)
}

fn xml_file(root: &Path, name: &str) -> PathBuf {
    root.join(format!("{name}.{XML_EXTENSION}"))
}

fn path_text(path: &Path) -> io::Result<&str> {
    path.to_str()
        .ok_or_else(|| invalid(format!("The path {} is not UTF-8", path.display())))
}

// A list of the store as its XML says it, if it lives in its own folder (`<root>/<name>/`, with
// its patch at `<root>/<name>/shelved.patch` and its copies inside) and its patch is there
// (`isValid`). Then nothing done to the list reaches another list's files.
fn own_list(root: &Path, name: &str, xml: XmlList) -> Option<ShelvedList> {
    if xml.name.as_deref() != Some(name) || !valid_name(name) {
        return None;
    }
    let folder = root.join(name);
    let patch = folder.join(PATCH_FILE);
    let real_folder = folder.symlink_metadata().is_ok_and(|meta| meta.is_dir());
    if !real_folder || xml.path.as_deref() != patch.to_str() || !regular_file(&patch) {
        return None;
    }
    let mut binaries = Vec::new();
    for binary in xml.binaries {
        let paths_valid = [&binary.before, &binary.after]
            .into_iter()
            .flatten()
            .all(|path| valid_relative(path));
        if !paths_valid || (binary.before.is_none() && binary.after.is_none()) {
            return None;
        }
        let shelved = match &binary.shelved {
            Some(value) => Some(inside(&folder, value)?),
            None => None,
        };
        binaries.push(ShelvedBinary {
            before: binary.before,
            after: binary.after,
            shelved,
        });
    }
    Some(ShelvedList {
        name: name.to_string(),
        description: xml.description.unwrap_or_default(),
        date_ms: xml.date,
        recycled: xml.recycled,
        to_delete: xml.to_delete,
        deleted: xml.deleted,
        patch,
        binaries,
    })
}

fn read_own(root: &Path, name: &str) -> Option<ShelvedList> {
    let file = xml_file(root, name);
    if !regular_file(&file) {
        return None;
    }
    let xml = read_xml(&fs::read(file).ok()?).ok()?.into_iter().next()?;
    own_list(root, name, xml)
}

// One list of the store by its name, read from its own XML.
pub(crate) fn find(data: &Path, name: &str) -> io::Result<ShelvedList> {
    if !valid_name(name) {
        return Err(invalid(format!(
            "\"{name}\" is not the name of a shelved changelist"
        )));
    }
    read_own(&shelf_root(data), name)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, list_gone(name)))
}

fn to_xml(list: &ShelvedList) -> io::Result<Vec<u8>> {
    let mut binaries = Vec::new();
    for binary in &list.binaries {
        let shelved = match &binary.shelved {
            Some(path) => Some(path_text(path)?.to_string()),
            None => None,
        };
        binaries.push(XmlBinary {
            before: binary.before.clone(),
            after: binary.after.clone(),
            shelved,
        });
    }
    Ok(write_xml(&XmlList {
        name: Some(list.name.clone()),
        date: list.date_ms,
        recycled: list.recycled,
        to_delete: list.to_delete,
        deleted: list.deleted,
        path: Some(path_text(&list.patch)?.to_string()),
        description: Some(list.description.clone()),
        binaries,
    }))
}

// Writes the list's XML. The caller holds the lock.
fn store(data: &Path, list: &ShelvedList) -> io::Result<()> {
    let root = shelf_root(data);
    let written = storage::write_atomic(&xml_file(&root, &list.name), &to_xml(list)?);
    changed(&root);
    written
}

fn write_patch(path: &Path, patch: &Patch) -> io::Result<()> {
    let options = WriteOptions {
        include_base: true,
        ..WriteOptions::default()
    };
    storage::write_atomic(path, &patch::write(patch, &options))
}

// The blob HEAD has at the before path of each binary file, for those it has.
fn head_ids(repo: &Path, binaries: &[BinaryChange]) -> BTreeMap<String, String> {
    binaries
        .iter()
        .filter_map(|binary| binary.before.as_deref())
        .filter_map(|before| Some((before.to_string(), head_blob(repo, before)?)))
        .collect()
}

// The blob id of `path` in HEAD, if it has one. The path was checked by the caller; it follows
// `HEAD:`, so it is never read as an option.
pub(crate) fn head_blob(repo: &Path, path: &str) -> Option<String> {
    let object = format!("HEAD:{path}");
    let output = git::run(repo, &["rev-parse", "--verify", "--quiet", &object]).ok()?;
    let id = String::from_utf8(output).ok()?.trim().to_string();
    (!id.is_empty()).then_some(id)
}

fn write_base_ids(folder: &Path, ids: &BTreeMap<String, String>) -> io::Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let json = serde_json::to_vec(ids).map_err(io::Error::other)?;
    storage::write_atomic(&folder.join(BASE_IDS_FILE), &json)
}

// What `create` recorded for the list's binary files: before path to blob id. Empty for a list
// that has no record.
pub fn base_ids(list: &ShelvedList) -> BTreeMap<String, String> {
    list.patch
        .parent()
        .and_then(|folder| fs::read(folder.join(BASE_IDS_FILE)).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

// Every list of the store, newest first (`ShelvedChangesViewManager`, by `DATE`).
pub fn list(data: &Path) -> Vec<ShelvedList> {
    let root = shelf_root(data);
    let Ok(entries) = fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut lists: Vec<ShelvedList> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let file_name = entry.file_name().into_string().ok()?;
            let name = file_name.strip_suffix(".xml")?;
            read_own(&root, name)
        })
        .collect();
    lists.sort_by(|a, b| b.date_ms.cmp(&a.date_ms).then_with(|| a.name.cmp(&b.name)));
    lists
}

// `createShelfFromChanges`: a new folder named after the description, the copies of the binary
// files, `shelved.patch` with the bases, then the XML. System lists (Update, rebase, checkout) are
// marked `toDelete`. What fails leaves nothing behind.
pub fn create(
    data: &Path,
    repo: &Path,
    description: &str,
    patch: &Patch,
    binaries: &[BinaryChange],
    system: bool,
) -> io::Result<ShelvedList> {
    for path in binaries
        .iter()
        .flat_map(|b| [&b.before, &b.after])
        .flatten()
    {
        snapshots::check_path(repo, path)?;
    }
    if binaries
        .iter()
        .any(|b| b.before.is_none() && b.after.is_none())
    {
        return Err(invalid("A binary change without paths"));
    }
    let _guard = lock();
    let root = shelf_root(data);
    fs::create_dir_all(&root)?;
    let name = unique_name(&root, description);
    if !valid_name(&name) {
        return Err(invalid(format!("No valid name for \"{description}\"")));
    }
    let folder = root.join(&name);
    fs::create_dir(&folder)?;
    let made = (|| {
        // Written first: the copies then take names beside it, never its own.
        write_base_ids(&folder, &head_ids(repo, binaries))?;
        let mut shelved = Vec::new();
        for binary in binaries {
            let copy = match &binary.after {
                Some(after) => {
                    let source = repo.join(after);
                    if !regular_file(&source) {
                        return Err(invalid(format!("\"{after}\" is not a file")));
                    }
                    Some(copy_into(&folder, file_name_of(after), &source)?)
                }
                None => None,
            };
            shelved.push(ShelvedBinary {
                before: binary.before.clone(),
                after: binary.after.clone(),
                shelved: copy,
            });
        }
        let list = ShelvedList {
            name: name.clone(),
            description: description.replace('\n', " "),
            date_ms: millis(SystemTime::now()),
            recycled: false,
            to_delete: system,
            deleted: false,
            patch: folder.join(PATCH_FILE),
            binaries: shelved,
        };
        write_patch(&list.patch, patch)?;
        store(data, &list)?;
        Ok(list)
    })();
    if made.is_err() {
        let _ = fs::remove_file(xml_file(&root, &name));
        let _ = fs::remove_dir_all(&folder);
    }
    made
}

// `importChangeLists` for one file: the patch is copied as it is into a new list named after
// `description`, dated `date_ms` (the file's own date). The caller has read it as a patch.
pub fn add_patch_file(
    data: &Path,
    description: &str,
    bytes: &[u8],
    date_ms: u64,
) -> io::Result<ShelvedList> {
    let _guard = lock();
    let root = shelf_root(data);
    fs::create_dir_all(&root)?;
    let name = unique_name(&root, description);
    if !valid_name(&name) {
        return Err(invalid(format!("No valid name for \"{description}\"")));
    }
    let folder = root.join(&name);
    fs::create_dir(&folder)?;
    let list = ShelvedList {
        name: name.clone(),
        description: description.replace('\n', " "),
        date_ms,
        recycled: false,
        to_delete: false,
        deleted: false,
        patch: folder.join(PATCH_FILE),
        binaries: Vec::new(),
    };
    let made = storage::write_atomic(&list.patch, bytes).and_then(|()| store(data, &list));
    if made.is_err() {
        let _ = fs::remove_file(xml_file(&root, &name));
        let _ = fs::remove_dir_all(&folder);
    }
    made.map(|()| list)
}

pub(crate) fn patch_error(error: PatchError) -> io::Error {
    match error {
        PatchError::Syntax { line, message } => invalid(format!("{message} (line {})", line + 1)),
        PatchError::Git(error) => io::Error::other(ApiError::from(error).message),
        PatchError::BinaryAsText { path } => invalid(format!("\"{path}\" is not text")),
    }
}

fn read_patch(path: &Path) -> io::Result<Patch> {
    patch::read(&fs::read(path)?).map_err(patch_error)
}

// The list's patch, with the stored bases (LF).
pub fn load_patch(data: &Path, name: &str) -> io::Result<Patch> {
    read_patch(&find(data, name)?.patch)
}

// `recycleChangeList`: applied, dated now; a system list becomes deleted.
fn mark_recycled(list: &mut ShelvedList, now: SystemTime) {
    list.recycled = true;
    list.date_ms = millis(now);
    if list.to_delete {
        list.to_delete = false;
        list.deleted = true;
    }
}

// `markChangeListAsDeleted`.
fn mark_deleted(list: &mut ShelvedList, now: SystemTime) {
    list.deleted = true;
    list.date_ms = millis(now);
}

pub fn recycle(data: &Path, name: &str, now: SystemTime) -> io::Result<ShelvedList> {
    let _guard = lock();
    let mut list = find(data, name)?;
    mark_recycled(&mut list, now);
    store(data, &list)?;
    Ok(list)
}

fn names(file: &FilePatch) -> (Option<&str>, Option<&str>) {
    match file {
        FilePatch::Text(text) => (text.before.as_deref(), text.after.as_deref()),
        FilePatch::Binary(binary) => (binary.before.as_deref(), binary.after.as_deref()),
    }
}

// rama's own: a new folder in `<data>/shelf-erased/` named after the moment and the list, for
// what the store erases.
fn erased_place(root: &Path, name: &str, now: SystemTime) -> io::Result<PathBuf> {
    let base = root.parent().unwrap_or(root).join(ERASED_DIR);
    fs::create_dir_all(&base)?;
    let mut cut = name.len().min(ERASED_NAME_BYTES);
    while !name.is_char_boundary(cut) {
        cut -= 1;
    }
    let stem = format!("{}-{}", millis(now), &name[..cut]);
    let mut number = 0;
    loop {
        let place = match number {
            0 => base.join(&stem),
            _ => base.join(format!("{stem}-{number}")),
        };
        match fs::create_dir(&place) {
            Ok(()) => return Ok(place),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => number += 1,
            Err(error) => return Err(error),
        }
    }
}

fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

// A copy of the list as it is now (its folder and its XML) in `<data>/shelf-erased/`, before the
// store takes files out of it for good.
fn keep_erased_copy(root: &Path, list: &ShelvedList, now: SystemTime) -> io::Result<()> {
    let place = erased_place(root, &list.name, now)?;
    copy_tree(&root.join(&list.name), &place.join(&list.name))?;
    fs::copy(xml_file(root, &list.name), xml_file(&place, &list.name))?;
    Ok(())
}

// `deleteResources`: the list is gone from the store. rama moves its folder and its XML to
// `<data>/shelf-erased/` instead of deleting them, and the cleanup removes them after
// `snapshots::SNAPSHOT_DAYS` days. A link in the place of the folder is moved, never followed.
fn erase(root: &Path, list: &ShelvedList, now: SystemTime) -> io::Result<()> {
    let place = erased_place(root, &list.name, now)?;
    let folder = root.join(&list.name);
    if folder.symlink_metadata().is_ok() {
        fs::rename(&folder, place.join(&list.name))?;
    }
    let moved = fs::rename(xml_file(root, &list.name), xml_file(&place, &list.name));
    changed(root);
    moved
}

// The erased lists older than `snapshots::SNAPSHOT_DAYS` days. A folder whose name does not start
// with its moment is not the store's and is left alone.
fn prune_erased(root: &Path, now: SystemTime) -> io::Result<()> {
    let base = root.parent().unwrap_or(root).join(ERASED_DIR);
    let entries = match fs::read_dir(&base) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let cutoff = millis(now).saturating_sub(snapshots::SNAPSHOT_DAYS * DAY_MS);
    let mut first_error = None;
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        let moment = name
            .to_str()
            .and_then(|name| name.split_once('-'))
            .and_then(|(moment, _)| moment.parse::<u64>().ok());
        let is_folder = entry.file_type().is_ok_and(|kind| kind.is_dir());
        if !is_folder || moment.is_none_or(|moment| moment >= cutoff) {
            continue;
        }
        if let Err(error) = fs::remove_dir_all(entry.path()) {
            first_error = first_error.or(Some(error));
        }
    }
    first_error.map_or(Ok(()), Err)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Removed {
    Applied,
    Deleted,
}

// `saveRemainingPatchesIfNeeded`. `remaining` and `remaining_binaries` are what stays in the list.
fn save_remaining(
    data: &Path,
    name: &str,
    remaining: &Patch,
    remaining_binaries: &[ShelvedBinary],
    now: SystemTime,
    removed: Removed,
) -> io::Result<Option<ShelvedList>> {
    let _guard = lock();
    let root = shelf_root(data);
    let mut list = find(data, name)?;
    // The binaries that stay are the list's own. The kept list and the copy then split the list's
    // binaries by the same test, and none is lost between them.
    if let Some(foreign) = remaining_binaries
        .iter()
        .find(|binary| !list.binaries.contains(binary))
    {
        let path = foreign.after.as_ref().or(foreign.before.as_ref());
        return Err(invalid(format!(
            "\"{}\" is not a binary file of this shelved changelist",
            path.map_or("", String::as_str)
        )));
    }
    if remaining.files.is_empty() && remaining_binaries.is_empty() {
        match removed {
            Removed::Applied => mark_recycled(&mut list, now),
            Removed::Deleted if list.deleted => {
                erase(&root, &list, now)?;
                return Ok(None);
            }
            Removed::Deleted => mark_deleted(&mut list, now),
        }
        store(data, &list)?;
        return Ok(None);
    }
    if removed == Removed::Applied && list.recycled {
        return Ok(None);
    }
    let original = read_patch(&list.patch)?;
    let stays = |file: &FilePatch| {
        remaining
            .files
            .iter()
            .any(|kept| names(kept) == names(file))
    };
    let mut kept_list = list.clone();
    kept_list
        .binaries
        .retain(|binary| remaining_binaries.contains(binary));
    if removed == Removed::Deleted && list.deleted {
        // `saveRemainingChangesInList` alone. What it takes out goes nowhere else: the list as it
        // was is kept with the erased ones first.
        keep_erased_copy(&root, &list, now)?;
        write_patch(&list.patch, remaining)?;
        store(data, &kept_list)?;
        return Ok(None);
    }

    // `saveRemainingAndRecycleOthers`. The copy, with what was applied or deleted, is written in
    // full before the list loses it.
    let copy_name = unique_name(&root, &list.description);
    let folder = root.join(&copy_name);
    fs::create_dir(&folder)?;
    let copied = (|| -> io::Result<Option<ShelvedList>> {
        write_base_ids(&folder, &base_ids(&list))?;
        let mut binaries = Vec::new();
        for binary in &list.binaries {
            if remaining_binaries.contains(binary) {
                continue;
            }
            let shelved = match (&binary.shelved, &binary.after) {
                (Some(source), Some(after)) if regular_file(source) => {
                    Some(copy_into(&folder, file_name_of(after), source)?)
                }
                // A copy that is gone has nothing to carry (`copyBinaryFiles`).
                (Some(_), _) => continue,
                (None, _) => None,
            };
            binaries.push(ShelvedBinary {
                shelved,
                ..binary.clone()
            });
        }
        let files: Vec<FilePatch> = original
            .files
            .iter()
            .filter(|file| !stays(file))
            .cloned()
            .collect();
        if files.is_empty() && binaries.is_empty() {
            return Ok(None);
        }
        let mut copy = ShelvedList {
            name: copy_name.clone(),
            patch: folder.join(PATCH_FILE),
            binaries,
            ..list.clone()
        };
        match removed {
            Removed::Applied => mark_recycled(&mut copy, now),
            Removed::Deleted => mark_deleted(&mut copy, now),
        }
        let applied = Patch {
            files,
            ..original.clone()
        };
        write_patch(&copy.patch, &applied)?;
        store(data, &copy)?;
        Ok(Some(copy))
    })();
    match &copied {
        Ok(Some(_)) => {}
        _ => {
            let _ = fs::remove_file(xml_file(&root, &copy_name));
            let _ = fs::remove_dir_all(&folder);
        }
    }
    let copy = copied?;
    write_patch(&list.patch, remaining)?;
    store(data, &kept_list)?;
    Ok(copy)
}

// After an unshelve with `Remove successfully applied files from the shelf`: everything applied
// recycles the list; otherwise a recycled copy takes the applied part and the list keeps the rest,
// unless it was recycled already.
pub fn save_remaining_and_recycle_applied(
    data: &Path,
    name: &str,
    remaining: &Patch,
    remaining_binaries: &[ShelvedBinary],
    now: SystemTime,
) -> io::Result<Option<ShelvedList>> {
    save_remaining(
        data,
        name,
        remaining,
        remaining_binaries,
        now,
        Removed::Applied,
    )
}

// Delete of some files of a list (`removeChangesFromChangeList`): a deleted copy takes them, or the
// list just loses them when it is deleted already.
pub fn save_remaining_and_delete_others(
    data: &Path,
    name: &str,
    remaining: &Patch,
    remaining_binaries: &[ShelvedBinary],
    now: SystemTime,
) -> io::Result<Option<ShelvedList>> {
    save_remaining(
        data,
        name,
        remaining,
        remaining_binaries,
        now,
        Removed::Deleted,
    )
}

// `deleteShelves` for a whole list: marked deleted and dated now, or erased if it was deleted
// already. The deleted list is returned; `None` when it was erased.
pub fn delete(data: &Path, name: &str, now: SystemTime) -> io::Result<Option<ShelvedList>> {
    let _guard = lock();
    let mut list = find(data, name)?;
    if list.deleted {
        erase(&shelf_root(data), &list, now)?;
        return Ok(None);
    }
    mark_deleted(&mut list, now);
    store(data, &list)?;
    Ok(Some(list))
}

// `restoreList`: no longer deleted, with the given date (Undo gives the original one).
pub fn restore(data: &Path, name: &str, date_ms: u64) -> io::Result<ShelvedList> {
    let _guard = lock();
    let mut list = find(data, name)?;
    list.deleted = false;
    list.date_ms = date_ms;
    store(data, &list)?;
    Ok(list)
}

// `renameChangeList`: the description only; the name and the folder stay.
pub fn rename(data: &Path, name: &str, description: &str) -> io::Result<ShelvedList> {
    let _guard = lock();
    let mut list = find(data, name)?;
    list.description = description.to_string();
    store(data, &list)?;
    Ok(list)
}

// `markDeletedSystemUnshelved` and `cleanDeletedOlderOneWeek`, on open and then once a day. Returns
// how many lists were erased.
pub fn cleanup(data: &Path, now: SystemTime) -> io::Result<usize> {
    let _guard = lock();
    let root = shelf_root(data);
    let cutoff = millis(now).saturating_sub(DELETED_KEPT_MS);
    let mut erased = 0;
    let mut first_error = None;
    for mut shelved in list(data) {
        let result = if shelved.recycled && shelved.to_delete {
            shelved.deleted = true;
            shelved.to_delete = false;
            store(data, &shelved)
        } else {
            Ok(())
        };
        let result = result.and_then(|()| {
            if shelved.deleted && shelved.date_ms < cutoff {
                erase(&root, &shelved, now)?;
                erased += 1;
            }
            Ok(())
        });
        if let Err(error) = result {
            first_error = first_error.or(Some(error));
        }
    }
    if let Err(error) = prune_erased(&root, now) {
        first_error = first_error.or(Some(error));
    }
    first_error.map_or(Ok(erased), Err)
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

// A list of `.idea/shelf` that can be copied: its patch and its binary copies, checked.
struct Source {
    xml: XmlList,
    patch: Vec<u8>,
    binaries: Vec<(XmlBinary, Option<PathBuf>)>,
    key: ImportKey,
}

// IntelliJ writes `$PROJECT_DIR$/…` for what is inside the project; a custom shelf keeps absolute
// paths.
fn expand(project: &Path, value: &str) -> Option<PathBuf> {
    match value.strip_prefix(PROJECT_DIR_PREFIX) {
        Some(rest) => Some(project.join(rest)),
        None => Path::new(value).is_absolute().then(|| PathBuf::from(value)),
    }
}

// The real path of a regular file (not a link) inside `folder`.
fn real_file_inside(folder: &Path, path: &Path) -> Option<PathBuf> {
    if !regular_file(path) {
        return None;
    }
    let real = fs::canonicalize(path).ok()?;
    (real.starts_with(folder) && real != folder).then_some(real)
}

fn source(project: &Path, shelf: &Path, xml: XmlList) -> Option<Source> {
    let patch_path = real_file_inside(shelf, &expand(project, xml.path.as_deref()?)?)?;
    let folder = patch_path.parent()?.to_path_buf();
    let mut binaries = Vec::new();
    for binary in &xml.binaries {
        let mut binary = binary.clone();
        for path in [&mut binary.before, &mut binary.after] {
            if let Some(value) = path.as_mut() {
                if let Some(rest) = value.strip_prefix(PROJECT_DIR_PREFIX) {
                    *value = rest.to_string();
                }
                if !valid_relative(value) {
                    return None;
                }
            }
        }
        if binary.before.is_none() && binary.after.is_none() {
            return None;
        }
        let copy = match &binary.shelved {
            Some(value) => Some(real_file_inside(&folder, &expand(project, value)?)?),
            None => None,
        };
        binaries.push((binary, copy));
    }
    let patch = fs::read(&patch_path).ok()?;
    let key = ImportKey {
        description: xml.description.clone().unwrap_or_default(),
        date: xml.date,
        sha256: sha256(&patch),
        binaries: Some(binaries_key(binaries.iter().map(|(binary, copy)| {
            (&binary.before, &binary.after, copy.as_deref())
        }))),
    };
    Some(Source {
        xml,
        patch,
        binaries,
        key,
    })
}

fn key_of(list: &ShelvedList) -> Option<ImportKey> {
    Some(ImportKey {
        description: list.description.clone(),
        date: list.date_ms,
        sha256: sha256(&fs::read(&list.patch).ok()?),
        binaries: Some(binaries_key(list.binaries.iter().map(|binary| {
            (&binary.before, &binary.after, binary.shelved.as_deref())
        }))),
    })
}

// Copies one list into the store under a name of its own: the patch as it is, the binary copies,
// then the XML, with the date and the states of the original.
fn copy_source(data: &Path, root: &Path, source: &Source) -> io::Result<()> {
    let name = unique_name(root, &source.key.description);
    let folder = root.join(&name);
    fs::create_dir(&folder)?;
    let made = (|| {
        let patch = folder.join(PATCH_FILE);
        storage::write_atomic(&patch, &source.patch)?;
        let mut binaries = Vec::new();
        for (binary, copy) in &source.binaries {
            let shelved = match copy {
                Some(from) => {
                    let file_name = from
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("binary");
                    Some(copy_into(&folder, file_name, from)?)
                }
                None => None,
            };
            binaries.push(ShelvedBinary {
                before: binary.before.clone(),
                after: binary.after.clone(),
                shelved,
            });
        }
        store(
            data,
            &ShelvedList {
                name: name.clone(),
                description: source.key.description.clone(),
                date_ms: source.xml.date,
                recycled: source.xml.recycled,
                to_delete: source.xml.to_delete,
                deleted: source.xml.deleted,
                patch,
                binaries,
            },
        )
    })();
    if made.is_err() {
        let _ = fs::remove_file(xml_file(root, &name));
        let _ = fs::remove_dir_all(&folder);
    }
    made
}

// Import from IntelliJ's shelf of the project (`.idea/shelf`, or the custom path of
// `workspace.xml`): read-only there. A list is recognised by its description, date and the
// SHA-256 of its patch, against the lists of the store and the record of earlier imports, so it
// is imported once even if it was deleted here since.
pub fn import(data: &Path, repo: &Path) -> io::Result<ImportResult> {
    let shelf = idea_import::shelf_folder(repo).map_err(io::Error::other)?;
    let mut result = ImportResult::default();
    let Ok(shelf) = fs::canonicalize(&shelf) else {
        return Ok(result);
    };
    let Ok(entries) = fs::read_dir(&shelf) else {
        return Ok(result);
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == XML_EXTENSION) && regular_file(path)
        })
        .collect();
    files.sort();

    let _guard = lock();
    let root = shelf_root(data);
    fs::create_dir_all(&root)?;
    let mut imported: Imported = storage::load_json(&root, IMPORTED_FILE, IMPORTED_CORRUPT)?;
    let mut known: Vec<ImportKey> = imported.lists.clone();
    known.extend(list(data).iter().filter_map(key_of));
    let mut first_error = None;
    for file in files {
        let lists = fs::read(&file)
            .ok()
            .and_then(|bytes| read_xml(&bytes).ok())
            .unwrap_or_default();
        if lists.is_empty() {
            result.skipped += 1;
        }
        for xml in lists {
            let Some(source) = source(repo, &shelf, xml) else {
                result.skipped += 1;
                continue;
            };
            if known.iter().any(|key| key.same(&source.key)) {
                result.skipped += 1;
                continue;
            }
            match copy_source(data, &root, &source) {
                Ok(()) => {
                    result.added += 1;
                    known.push(source.key.clone());
                    imported.lists.push(source.key);
                }
                Err(error) => first_error = first_error.or(Some(error)),
            }
        }
    }
    let record = serde_json::to_vec_pretty(&imported).map_err(io::Error::other)?;
    storage::write_atomic(&root.join(IMPORTED_FILE), &record)?;
    first_error.map_or(Ok(result), Err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch::{self, FilePatch, PatchSource, TextFilePatch, WriteOptions};
    use crate::test_support::TempRepo;
    use std::collections::BTreeMap;
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::time::{Duration, UNIX_EPOCH};
    use tempfile::TempDir;

    const DAY: u64 = 24 * 60 * 60 * 1000;
    const NOW: u64 = 1_700_000_000_000;

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/shelf")
    }

    fn files_under(dir: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let mut pending = vec![dir.to_path_buf()];
        while let Some(folder) = pending.pop() {
            for entry in fs::read_dir(folder).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    found.push(path);
                }
            }
        }
        found.sort();
        found
    }

    // Every file and link under `dir`, with its bytes (or target) and its modification time.
    fn snapshot(dir: &Path) -> BTreeMap<PathBuf, (Vec<u8>, SystemTime)> {
        let mut found = BTreeMap::new();
        let mut pending = vec![dir.to_path_buf()];
        while let Some(folder) = pending.pop() {
            for entry in fs::read_dir(folder).unwrap() {
                let path = entry.unwrap().path();
                let meta = path.symlink_metadata().unwrap();
                let bytes = if meta.file_type().is_symlink() {
                    fs::read_link(&path)
                        .unwrap()
                        .into_os_string()
                        .into_encoded_bytes()
                } else if meta.is_dir() {
                    pending.push(path.clone());
                    Vec::new()
                } else {
                    fs::read(&path).unwrap()
                };
                found.insert(path, (bytes, meta.modified().unwrap()));
            }
        }
        found
    }

    fn at(ms: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(ms)
    }

    fn side(path: &str, content: &[u8]) -> Option<(String, Vec<u8>)> {
        Some((path.to_string(), content.to_vec()))
    }

    fn modified(path: &str, before: &[u8], after: &[u8]) -> PatchSource {
        PatchSource {
            before: side(path, before),
            after: side(path, after),
            before_rev: Some("1".to_string()),
            binary: false,
        }
    }

    // As Shelve builds it: with the base of the modified files.
    fn built(repo: &Path, entries: &[PatchSource]) -> Patch {
        let options = WriteOptions {
            include_base: true,
            ..WriteOptions::default()
        };
        patch::build(repo, entries, &options).unwrap()
    }

    // A git patch takes its version ids from the `index` line alone (`GitPatchParser`), so the
    // `(revision …)` and `(date …)` written by Shelve do not come back, in IntelliJ either.
    fn without_versions(mut patch: Patch) -> Patch {
        for file in &mut patch.files {
            if let FilePatch::Text(text) = file {
                text.before_rev = None;
                text.after_rev = None;
            }
        }
        patch
    }

    fn text(file: &FilePatch) -> &TextFilePatch {
        match file {
            FilePatch::Text(text) => text,
            FilePatch::Binary(_) => panic!("a binary file patch"),
        }
    }

    fn shelf_of(data: &Path, name: &str) -> ShelvedList {
        list(data)
            .into_iter()
            .find(|list| list.name == name)
            .unwrap_or_else(|| panic!("no list {name}"))
    }

    fn binary(before: Option<&str>, after: Option<&str>) -> BinaryChange {
        BinaryChange {
            before: before.map(str::to_string),
            after: after.map(str::to_string),
        }
    }

    fn some(text: &str) -> Option<String> {
        Some(text.to_string())
    }

    // Copies a fixture folder, with its XML paths moved from `.shelf` (the fixtures are of an
    // `.ipr` project) to `to`.
    fn copy_fixture(from: &Path, to: &Path, prefix: &str) {
        fs::create_dir_all(to).unwrap();
        for file in files_under(from) {
            let target = to.join(file.strip_prefix(from).unwrap());
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            if file.extension().is_some_and(|ext| ext == "xml") {
                let xml = fs::read_to_string(&file)
                    .unwrap()
                    .replace("$PROJECT_DIR$/.shelf/", prefix);
                fs::write(target, xml).unwrap();
            } else {
                fs::copy(&file, target).unwrap();
            }
        }
    }

    const NO_CHANGELISTS: &str =
        r#"<project version="4"><component name="ChangeListManager" /></project>"#;

    // `SHELF_TOPIC`: every change to the store moves its generation, so that `repo-changed` says
    // the shelf changed; reading it, or another store, does not.
    #[test]
    fn every_change_to_the_store_moves_its_generation_and_a_read_does_not() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\n", "a");
        let data = TempDir::new().unwrap();
        let other = TempDir::new().unwrap();
        let patch = built(repo.path(), &[modified("a.txt", b"one\n", b"two\n")]);
        let mut seen = generation(data.path());
        let mut moved = |what: &str| {
            let now = generation(data.path());
            assert_ne!(now, seen, "{what}");
            seen = now;
        };

        let made = create(data.path(), repo.path(), "Work", &patch, &[], false).unwrap();
        moved("create");
        rename(data.path(), &made.name, "Other").unwrap();
        moved("rename");
        recycle(data.path(), &made.name, at(NOW)).unwrap();
        moved("recycle");
        delete(data.path(), &made.name, at(NOW)).unwrap();
        moved("delete");
        restore(data.path(), &made.name, NOW).unwrap();
        moved("restore");
        add_patch_file(data.path(), "Imported", b"--- a/x\n+++ b/x\n", NOW).unwrap();
        moved("add_patch_file");
        delete(data.path(), &made.name, at(NOW - 30 * DAY)).unwrap();
        cleanup(data.path(), at(NOW)).unwrap();
        moved("delete and cleanup");

        let before = generation(data.path());
        let _ = list(data.path());
        let _ = tab::entries(data.path());
        let _ = load_patch(data.path(), "Imported");
        create(other.path(), repo.path(), "Elsewhere", &patch, &[], false).unwrap();
        cleanup(data.path(), at(NOW)).unwrap();
        assert_eq!(generation(data.path()), before);
    }

    #[test]
    fn reads_every_intellij_shelf_fixture() {
        let (mut xml_files, mut patch_files) = (0, 0);
        for file in files_under(&fixtures()) {
            let bytes = fs::read(&file).unwrap();
            match file.extension().and_then(|ext| ext.to_str()) {
                Some("xml") => {
                    let lists =
                        read_xml(&bytes).unwrap_or_else(|error| panic!("{file:?}: {error}"));
                    assert!(!lists.is_empty(), "{file:?}");
                    for list in &lists {
                        let path = list.path.as_deref().unwrap();
                        assert!(path.starts_with("$PROJECT_DIR$/.shelf/"), "{file:?}");
                        assert!(list.description.is_some() && list.date > 0, "{file:?}");
                    }
                    xml_files += 1;
                }
                Some("patch") => {
                    let read = patch::read(&bytes).unwrap();
                    assert!(!read.files.is_empty(), "{file:?}");
                    patch_files += 1;
                }
                _ => {}
            }
        }
        assert_eq!((xml_files, patch_files), (24, 29));

        let read = |path: &str| read_xml(&fs::read(fixtures().join(path)).unwrap()).unwrap();
        let shelved = |name: &str| some(&format!("$PROJECT_DIR$/.shelf/test/{name}"));
        assert_eq!(
            read("shelvedChangeLists/test.xml"),
            [XmlList {
                name: some("test"),
                date: 1539026452817,
                recycled: false,
                to_delete: false,
                deleted: false,
                path: some("$PROJECT_DIR$/.shelf/test/shelved.patch"),
                description: some("description"),
                binaries: vec![
                    XmlBinary {
                        before: None,
                        after: some("c.png"),
                        shelved: shelved("c.png"),
                    },
                    XmlBinary {
                        before: None,
                        after: some("c1.png"),
                        shelved: shelved("c1.png"),
                    },
                ],
            }]
        );
        // The old `workspace.xml` component: the element says whether the list is recycled
        // (`migrateOldShelfInfo`), and the lists have no name yet.
        let flags = |path: &str| -> Vec<_> {
            read(path)
                .into_iter()
                .map(|list| (list.name, list.recycled, list.to_delete, list.deleted))
                .collect()
        };
        assert_eq!(
            flags("defaultCleaningNothingToDelete/before.xml"),
            [(None, true, false, false), (None, false, true, false)]
        );
        assert_eq!(
            flags("migrateInfoRecycled/before.xml"),
            [(None, true, false, false)]
        );
        assert_eq!(
            flags("cleaningWeek/after/test2.xml"),
            [(some("test2"), true, true, false)]
        );
        let binaries = &read("migrateInfoWithBinaries/before.xml")[0].binaries;
        assert_eq!(binaries[0].before, some("$PROJECT_DIR$/.shelf/c_old.png"));
        let long = read("migrateInfoWithVeryLongDescription/after/test_test_test_test_test_test_test_test_test_test_test_test_test_test_test_test_test_test_.xml");
        assert_eq!(long[0].name, Some("test_".repeat(18)));
        assert_eq!(long[0].description, Some("test ".repeat(27)));

        let patch = patch::read(
            &fs::read(fixtures().join("shelvedChangeLists/test/shelved.patch")).unwrap(),
        )
        .unwrap();
        let names: Vec<_> = patch
            .files
            .iter()
            .map(|file| (text(file).before.clone(), text(file).after.clone()))
            .collect();
        assert_eq!(names, [(None, some("1.txt")), (None, some("2.txt"))]);
    }

    #[test]
    fn writes_xml_byte_identical_to_the_fixtures() {
        let mut checked = 0;
        for file in files_under(&fixtures()) {
            let in_after = file.parent().unwrap().file_name().unwrap() == "after";
            if !in_after || file.extension().unwrap() != "xml" {
                continue;
            }
            let bytes = fs::read(&file).unwrap();
            let lists = read_xml(&bytes).unwrap();
            assert_eq!(lists.len(), 1, "{file:?}");
            assert_eq!(
                String::from_utf8(write_xml(&lists[0])).unwrap(),
                String::from_utf8(bytes).unwrap(),
                "{file:?}"
            );
            checked += 1;
        }
        assert_eq!(checked, 11);

        // `JDOMUtil.escapeText(value, false, true)`; an absent value is an option without one. A
        // character XML cannot hold is dropped.
        let list = XmlList {
            name: some("n"),
            date: 5,
            recycled: true,
            to_delete: false,
            deleted: true,
            path: some("/s/n/shelved.patch"),
            description: some("a<b>&\"c\"'\t\n\r\u{1}é😀"),
            binaries: vec![XmlBinary {
                before: None,
                after: some("a b.png"),
                shelved: None,
            }],
        };
        let expected = "<changelist name=\"n\" date=\"5\" recycled=\"true\" deleted=\"true\">\n  \
            <option name=\"PATH\" value=\"/s/n/shelved.patch\" />\n  \
            <option name=\"DESCRIPTION\" value=\"a&lt;b&gt;&amp;&quot;c&quot;'&#9;&#10;&#13;é😀\" />\n  \
            <binary>\n    \
            <option name=\"BEFORE_PATH\" />\n    \
            <option name=\"AFTER_PATH\" value=\"a b.png\" />\n    \
            <option name=\"SHELVED_PATH\" />\n  \
            </binary>\n\
            </changelist>";
        assert_eq!(String::from_utf8(write_xml(&list)).unwrap(), expected);
        let mut back = list.clone();
        back.description = some("a<b>&\"c\"'\t\n\ré😀");
        assert_eq!(read_xml(expected.as_bytes()).unwrap(), [back]);
    }

    // P10: the cases the patch file name (`src/patch/flow.ts` `patchNamePrefix`) is tested with too.
    #[test]
    fn the_name_prefix_follows_the_shared_cases() {
        let text = include_str!("../testdata/patch-names.json");
        let shared: serde_json::Value = serde_json::from_str(text).unwrap();
        for case in shared["cases"].as_array().unwrap() {
            let message = case["message"].as_str().unwrap();
            assert_eq!(
                shorten_and_sanitize(message),
                case["prefix"].as_str().unwrap(),
                "{message}"
            );
        }
    }

    #[test]
    fn names_sanitise_cut_and_number() {
        // `PathUtilRt.suggestFileName`: what Java's `Character.isWhitespace` calls a space goes,
        // a no-break space and NEL stay.
        assert_eq!(
            suggest_file_name("Fix v1.2: a/b\\c<d>e\"f|g?h*i;j\tk\u{1}l\u{2003}m\u{a0}n\u{85}o"),
            "Fix_v1_2__a_b_c_d_e_f_g_h_i_j_k_l_m\u{a0}n\u{85}o"
        );

        let data = TempDir::new().unwrap();
        let repo = TempDir::new().unwrap();
        let long = "test ".repeat(27);
        let emoji = format!("{}😀", "a".repeat(89));
        let cjk = "变".repeat(90);
        let names: Vec<String> = [
            "Fix v1.2",
            "Fix v1.2",
            "Fix v1.2",
            "",
            "../..",
            long.as_str(),
            emoji.as_str(),
            cjk.as_str(),
            cjk.as_str(),
        ]
        .iter()
        .map(|description| {
            create(
                data.path(),
                repo.path(),
                description,
                &Patch::default(),
                &[],
                false,
            )
            .unwrap()
            .name
        })
        .collect();
        assert_eq!(
            names,
            [
                "Fix_v1_2".to_string(),
                "Fix_v1_21".to_string(),
                "Fix_v1_22".to_string(),
                "unnamed".to_string(),
                "_____".to_string(),
                "test_".repeat(18),
                // Cut at 90 UTF-16 units, without splitting the emoji.
                "a".repeat(89),
                // And at 220 UTF-8 bytes on a character boundary, so that a name with its number
                // and `.xml` fits the 255 bytes of a file name.
                "变".repeat(73),
                format!("{}1", "变".repeat(73)),
            ]
        );
        assert!(names.iter().all(|name| valid_name(name)));
        let root = data.path().join("shelf");
        assert!(root.join("_____").is_dir() && root.join("_____.xml").is_file());

        // The newlines of the description become spaces (`commitMessage.replace('\n', ' ')`).
        let lines = create(
            data.path(),
            repo.path(),
            "one\ntwo",
            &Patch::default(),
            &[],
            false,
        )
        .unwrap();
        assert_eq!(
            (lines.name.as_str(), lines.description.as_str()),
            ("one_two", "one two")
        );

        // A name is free only if neither the folder nor the XML file exists.
        fs::write(root.join("taken.xml"), "").unwrap();
        fs::create_dir(root.join("held")).unwrap();
        let taken = |description| {
            create(
                data.path(),
                repo.path(),
                description,
                &Patch::default(),
                &[],
                false,
            )
            .unwrap()
            .name
        };
        assert_eq!(
            (taken("taken"), taken("held")),
            ("taken1".to_string(), "held1".to_string())
        );
    }

    #[test]
    fn binaries_are_copied_as_name_n_ext() {
        let data = TempDir::new().unwrap();
        let repo = TempDir::new().unwrap();
        let contents: [(&str, &[u8]); 4] = [
            ("a/c.png", &[1, 0, 2]),
            ("b/c.png", &[3, 0]),
            ("data", &[4, 0]),
            ("b/data", &[5, 0]),
        ];
        for (path, bytes) in contents {
            let file = repo.path().join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, bytes).unwrap();
        }
        let binaries = [
            binary(None, Some("a/c.png")),
            binary(Some("b/c.png"), Some("b/c.png")),
            binary(Some("data"), Some("data")),
            binary(Some("old/data"), Some("b/data")),
            binary(Some("gone.png"), None),
        ];
        let made = create(
            data.path(),
            repo.path(),
            "bins",
            &Patch::default(),
            &binaries,
            false,
        )
        .unwrap();
        let folder = data.path().join("shelf/bins");
        let shelved: Vec<_> = made
            .binaries
            .iter()
            .map(|binary| {
                binary.shelved.as_ref().map(|path| {
                    path.strip_prefix(&folder)
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .to_string()
                })
            })
            .collect();
        assert_eq!(
            shelved,
            [
                some("c.png"),
                some("c1.png"),
                some("data"),
                some("data1"),
                None
            ]
        );
        for (binary, (_, bytes)) in made.binaries.iter().zip(contents) {
            assert_eq!(fs::read(binary.shelved.as_ref().unwrap()).unwrap(), bytes);
        }
        assert_eq!(made.binaries[3].before, some("old/data"));
        assert_eq!(made.binaries[4].after, None);
        assert_eq!(list(data.path()), std::slice::from_ref(&made));

        let folder = folder.to_str().unwrap();
        let xml = fs::read_to_string(data.path().join("shelf/bins.xml")).unwrap();
        let start = format!(
            "<changelist name=\"bins\" date=\"{}\" recycled=\"false\">\n  \
             <option name=\"PATH\" value=\"{folder}/shelved.patch\" />\n  \
             <option name=\"DESCRIPTION\" value=\"bins\" />\n  \
             <binary>\n    \
             <option name=\"BEFORE_PATH\" />\n    \
             <option name=\"AFTER_PATH\" value=\"a/c.png\" />\n    \
             <option name=\"SHELVED_PATH\" value=\"{folder}/c.png\" />\n  \
             </binary>\n",
            made.date_ms
        );
        assert!(xml.starts_with(&start), "{xml}");
        let end = "  <binary>\n    \
             <option name=\"BEFORE_PATH\" value=\"gone.png\" />\n    \
             <option name=\"AFTER_PATH\" />\n    \
             <option name=\"SHELVED_PATH\" />\n  \
             </binary>\n</changelist>";
        assert!(xml.ends_with(end), "{xml}");

        // A path that leaves the repository is refused, and nothing is left behind.
        for path in ["../outside.png", "/etc/hosts"] {
            let bad = [binary(None, Some(path))];
            assert!(create(
                data.path(),
                repo.path(),
                "bad",
                &Patch::default(),
                &bad,
                false
            )
            .is_err());
        }
        assert_eq!(list(data.path()).len(), 1);
        assert!(!data.path().join("shelf/bad").exists());
        assert!(!data.path().join("shelf/bad.xml").exists());
    }

    #[test]
    fn recycle_and_partial_recycle_follow_241() {
        let data = TempDir::new().unwrap();
        let repo = TempDir::new().unwrap();
        for (path, bytes) in [
            ("one.txt", &b"1\nb\n"[..]),
            ("two.txt", b"2\nb\n"),
            ("x.png", &[0, 1]),
            ("y.png", &[0, 2]),
        ] {
            fs::write(repo.path().join(path), bytes).unwrap();
        }
        let patch = built(
            repo.path(),
            &[
                modified("one.txt", b"1\na\n", b"1\nb\n"),
                modified("two.txt", b"2\na\n", b"2\nb\n"),
            ],
        );
        assert!(text(&patch.files[1]).base.is_some());
        let bins = [
            binary(Some("x.png"), Some("x.png")),
            binary(Some("y.png"), Some("y.png")),
        ];
        let data = data.path();

        // Everything applied: the list itself is recycled, with the date of the unshelve.
        create(data, repo.path(), "whole", &patch, &bins, false).unwrap();
        assert_eq!(
            save_remaining_and_recycle_applied(data, "whole", &Patch::default(), &[], at(NOW))
                .unwrap(),
            None
        );
        let whole = shelf_of(data, "whole");
        assert!(whole.recycled && !whole.deleted && !whole.to_delete);
        assert_eq!(whole.date_ms, NOW);

        // A system list that is applied becomes deleted (`recycleChangeList`).
        let system = create(data, repo.path(), "system", &patch, &bins, true).unwrap();
        assert!(system.to_delete && !system.recycled);
        let system = recycle(data, "system", at(NOW)).unwrap();
        assert!(system.recycled && system.deleted && !system.to_delete);
        assert_eq!(system.date_ms, NOW);

        // Part applied: a recycled copy holds the applied part and the list keeps the rest, both
        // with their base.
        let part = create(data, repo.path(), "part", &patch, &bins, false).unwrap();
        let loaded = load_patch(data, "part").unwrap();
        assert_eq!(loaded, without_versions(patch.clone()));
        let remaining = Patch {
            files: vec![loaded.files[1].clone()],
            ..loaded.clone()
        };
        let copy = save_remaining_and_recycle_applied(
            data,
            "part",
            &remaining,
            &part.binaries[1..],
            at(NOW),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            (copy.name.as_str(), copy.description.as_str()),
            ("part1", "part")
        );
        assert!(copy.recycled && !copy.deleted);
        assert_eq!(copy.date_ms, NOW);
        assert_eq!(copy, shelf_of(data, "part1"));
        let applied = load_patch(data, "part1").unwrap();
        assert_eq!(applied.files, [loaded.files[0].clone()]);
        assert!(text(&applied.files[0]).base.is_some());
        assert_eq!(copy.binaries.len(), 1);
        assert_eq!(copy.binaries[0].after, some("x.png"));
        let copied = copy.binaries[0].shelved.clone().unwrap();
        assert!(copied.starts_with(data.join("shelf/part1")));
        assert_eq!(fs::read(copied).unwrap(), [0, 1]);

        let kept = shelf_of(data, "part");
        assert!(!kept.recycled && !kept.deleted);
        assert_eq!(kept.date_ms, part.date_ms);
        assert_eq!(kept.binaries, part.binaries[1..]);
        let rest = load_patch(data, "part").unwrap();
        assert_eq!(rest.files, [loaded.files[1].clone()]);
        assert!(text(&rest.files[0]).base.is_some());

        // A list already recycled does not change when only part of it is applied again.
        let before = (list(data), snapshot(&data.join("shelf")));
        let none = Patch::default();
        assert_eq!(
            save_remaining_and_recycle_applied(data, "part1", &none, &copy.binaries, at(NOW + 1))
                .unwrap(),
            None
        );
        assert_eq!((list(data), snapshot(&data.join("shelf"))), before);
    }

    #[test]
    fn delete_then_delete_erases() {
        let data = TempDir::new().unwrap();
        let repo = TempDir::new().unwrap();
        fs::write(repo.path().join("x.png"), [0, 1]).unwrap();
        fs::write(repo.path().join("one.txt"), "b\n").unwrap();
        let patch = built(repo.path(), &[modified("one.txt", b"a\n", b"b\n")]);
        let bins = [binary(None, Some("x.png"))];
        let data = data.path();
        let made = create(data, repo.path(), "gone", &patch, &bins, false).unwrap();
        create(data, repo.path(), "other", &patch, &bins, false).unwrap();

        let deleted = delete(data, "gone", at(NOW)).unwrap().unwrap();
        assert!(deleted.deleted && !deleted.recycled);
        assert_eq!(deleted.date_ms, NOW);
        assert_eq!(shelf_of(data, "gone"), deleted);
        assert!(made.patch.is_file() && made.binaries[0].shelved.as_ref().unwrap().is_file());

        assert_eq!(delete(data, "gone", at(NOW + 1)).unwrap(), None);
        let names: Vec<_> = list(data).into_iter().map(|list| list.name).collect();
        assert_eq!(names, ["other"]);
        let root = data.join("shelf");
        assert!(!root.join("gone").exists() && !root.join("gone.xml").exists());
        assert!(root.join("other/shelved.patch").is_file());
        assert!(delete(data, "gone", at(NOW)).is_err());
    }

    // Finding 5: what the store erases (Delete in Recently Deleted, the weekly cleanup, the files
    // taken out of a list deleted already) goes to `<data>/shelf-erased/`, kept 30 days.
    #[test]
    fn an_erased_list_is_kept_in_shelf_erased_for_thirty_days() {
        let data = TempDir::new().unwrap();
        let repo = TempDir::new().unwrap();
        fs::write(repo.path().join("x.png"), [0, 1]).unwrap();
        let patch = built(repo.path(), &[modified("one.txt", b"a\n", b"b\n")]);
        let bins = [binary(None, Some("x.png"))];
        let data = data.path();
        let made = create(data, repo.path(), "gone", &patch, &bins, false).unwrap();
        let patch_bytes = fs::read(&made.patch).unwrap();
        delete(data, "gone", at(NOW)).unwrap();

        assert_eq!(delete(data, "gone", at(NOW)).unwrap(), None);

        let kept = data.join(format!("shelf-erased/{NOW}-gone"));
        assert_eq!(
            fs::read(kept.join("gone/shelved.patch")).unwrap(),
            patch_bytes
        );
        assert_eq!(fs::read(kept.join("gone/x.png")).unwrap(), [0, 1]);
        assert!(kept.join("gone.xml").is_file());
        assert!(list(data).is_empty());
        cleanup(data, at(NOW + 29 * DAY)).unwrap();
        assert!(kept.is_dir());
        cleanup(data, at(NOW + 31 * DAY)).unwrap();
        assert!(!kept.exists());
    }

    #[test]
    fn files_taken_out_of_a_deleted_list_are_kept_in_shelf_erased() {
        let data = TempDir::new().unwrap();
        let repo = TempDir::new().unwrap();
        let patch = built(
            repo.path(),
            &[
                modified("one.txt", b"a\n", b"b\n"),
                modified("two.txt", b"c\n", b"d\n"),
            ],
        );
        let data = data.path();
        let made = create(data, repo.path(), "both", &patch, &[], false).unwrap();
        let whole = fs::read(&made.patch).unwrap();
        delete(data, "both", at(NOW)).unwrap();
        let loaded = load_patch(data, "both").unwrap();
        let second = Patch {
            files: vec![loaded.files[1].clone()],
            ..loaded.clone()
        };

        save_remaining_and_delete_others(data, "both", &second, &[], at(NOW + 1)).unwrap();

        assert_eq!(load_patch(data, "both").unwrap(), second);
        let kept = data.join(format!("shelf-erased/{}-both", NOW + 1));
        assert_eq!(fs::read(kept.join("both/shelved.patch")).unwrap(), whole);
        assert!(kept.join("both.xml").is_file());
    }

    // P11: a list whose folder is a link is not the store's: it is not listed, so nothing done to
    // it reaches the folder the link points at.
    #[test]
    fn a_list_whose_folder_is_a_link_is_not_read() {
        let data = TempDir::new().unwrap();
        let repo = TempDir::new().unwrap();
        let elsewhere = TempDir::new().unwrap();
        let data = data.path();
        let patch = built(repo.path(), &[modified("one.txt", b"a\n", b"b\n")]);
        create(data, repo.path(), "linked", &patch, &[], false).unwrap();
        let folder = data.join("shelf/linked");
        let target = elsewhere.path().join("linked");
        fs::rename(&folder, &target).unwrap();
        symlink(&target, &folder).unwrap();

        assert!(list(data).is_empty());
        assert!(delete(data, "linked", at(NOW)).is_err());
        assert!(target.join("shelved.patch").is_file());
    }

    // P14: a copy beside one whose name already takes 255 bytes still gets a name that fits.
    #[test]
    fn a_copy_of_a_file_with_a_name_of_255_bytes_gets_a_name_that_fits() {
        let folder = TempDir::new().unwrap();
        let source = folder.path().join("source");
        fs::write(&source, [1, 2]).unwrap();
        let long = format!("{}.png", "a".repeat(251));
        assert_eq!(long.len(), 255);

        let first = copy_into(folder.path(), &long, &source).unwrap();
        let second = copy_into(folder.path(), &long, &source).unwrap();
        let third = copy_into(folder.path(), &long, &source).unwrap();

        assert_eq!(first.file_name().unwrap().len(), 255);
        for copy in [&second, &third] {
            let name = copy.file_name().unwrap().to_str().unwrap();
            assert!(name.len() <= 255 && name.ends_with(".png"), "{name}");
            assert_eq!(fs::read(copy).unwrap(), [1, 2]);
        }
        assert_ne!(second, third);
    }

    // A name the store makes is always one it reads back: a long description in a script of three
    // UTF-8 bytes per character gives a list that is listed, loaded and deleted.
    #[test]
    fn a_long_cjk_description_gives_a_usable_list() {
        let data = TempDir::new().unwrap();
        let repo = TempDir::new().unwrap();
        fs::write(repo.path().join("one.txt"), "b\n").unwrap();
        fs::write(repo.path().join("x.png"), [1]).unwrap();
        let patch = built(repo.path(), &[modified("one.txt", b"a\n", b"b\n")]);
        let data = data.path();
        let description = "变更".repeat(60);
        let bins = [binary(Some("x.png"), Some("x.png"))];
        let made = create(data, repo.path(), &description, &patch, &bins, false).unwrap();
        assert!(made.name.len() <= 220);
        assert_eq!(made.description, description);
        assert_eq!(list(data), std::slice::from_ref(&made));
        assert_eq!(
            load_patch(data, &made.name).unwrap(),
            without_versions(patch)
        );
        assert!(delete(data, &made.name, at(NOW)).unwrap().is_some());
        assert_eq!(delete(data, &made.name, at(NOW)).unwrap(), None);
        assert!(list(data).is_empty());
        assert!(!data.join("shelf").join(&made.name).exists());
    }

    // The binaries that stay must be the list's own: anything else is refused before any write,
    // so a caller's mistake cannot drop a binary from both halves.
    #[test]
    fn remaining_binaries_must_be_the_lists_own() {
        let data = TempDir::new().unwrap();
        let repo = TempDir::new().unwrap();
        for (path, bytes) in [
            ("one.txt", &b"b\n"[..]),
            ("two.txt", b"d\n"),
            ("x.png", &[1]),
            ("y.png", &[2]),
        ] {
            fs::write(repo.path().join(path), bytes).unwrap();
        }
        let patch = built(
            repo.path(),
            &[
                modified("one.txt", b"a\n", b"b\n"),
                modified("two.txt", b"c\n", b"d\n"),
            ],
        );
        let data = data.path();
        let bins = [
            binary(Some("x.png"), Some("x.png")),
            binary(Some("y.png"), Some("y.png")),
        ];
        let made = create(data, repo.path(), "own", &patch, &bins, false).unwrap();
        let loaded = load_patch(data, "own").unwrap();
        let remaining = Patch {
            files: vec![loaded.files[1].clone()],
            ..loaded
        };
        // Same paths as the list's second binary, another copy.
        let foreign = ShelvedBinary {
            shelved: Some(data.join("shelf/own/other.png")),
            ..made.binaries[1].clone()
        };
        let before = (list(data), snapshot(&data.join("shelf")));
        for kept in [
            vec![foreign.clone()],
            vec![made.binaries[0].clone(), foreign],
        ] {
            assert!(
                save_remaining_and_recycle_applied(data, "own", &remaining, &kept, at(NOW))
                    .is_err()
            );
            assert!(
                save_remaining_and_delete_others(data, "own", &remaining, &kept, at(NOW)).is_err()
            );
        }
        assert_eq!((list(data), snapshot(&data.join("shelf"))), before);
    }

    // A list of the store lives in `<root>/<name>/`: an XML whose `PATH` points anywhere else (another
    // list's folder, the root itself) is not read, so nothing done to it reaches other files.
    #[test]
    fn a_store_list_is_read_only_from_its_own_folder() {
        let data = TempDir::new().unwrap();
        let repo = TempDir::new().unwrap();
        fs::write(repo.path().join("c.png"), [9]).unwrap();
        let data = data.path();
        let bins = [binary(None, Some("c.png"))];
        let a = create(data, repo.path(), "a", &Patch::default(), &bins, false).unwrap();
        create(data, repo.path(), "b", &Patch::default(), &bins, false).unwrap();
        let root = data.join("shelf");
        let a_copy = a.binaries[0].shelved.clone().unwrap();
        let tampered = |path: &Path, shelved: &Path| {
            format!(
                "<changelist name=\"b\" date=\"1\" recycled=\"false\" deleted=\"true\">\n  \
                 <option name=\"PATH\" value=\"{}\" />\n  \
                 <option name=\"DESCRIPTION\" value=\"b\" />\n  <binary>\n    \
                 <option name=\"AFTER_PATH\" value=\"c.png\" />\n    \
                 <option name=\"SHELVED_PATH\" value=\"{}\" />\n  </binary>\n</changelist>",
                path.display(),
                shelved.display()
            )
        };
        fs::write(root.join("shelved.patch"), "").unwrap();
        for (path, shelved) in [
            (root.join("a/shelved.patch"), a_copy.clone()),
            (root.join("shelved.patch"), root.join("a.xml")),
        ] {
            fs::write(root.join("b.xml"), tampered(&path, &shelved)).unwrap();
            let names: Vec<_> = list(data).into_iter().map(|list| list.name).collect();
            assert_eq!(names, ["a"]);
            assert!(delete(data, "b", at(NOW)).is_err());
            assert_eq!(cleanup(data, at(NOW)).unwrap(), 0);
        }
        assert!(a.patch.is_file() && a_copy.is_file() && root.join("a.xml").is_file());
        assert_eq!(shelf_of(data, "a"), a);
    }

    // `removeChangesFromChangeList`: a deleted copy takes the deleted files; a list deleted
    // already only loses them.
    #[test]
    fn partial_delete_follows_241() {
        let data = TempDir::new().unwrap();
        let repo = TempDir::new().unwrap();
        for (path, bytes) in [
            ("one.txt", &b"b\n"[..]),
            ("two.txt", b"d\n"),
            ("x.png", &[7]),
        ] {
            fs::write(repo.path().join(path), bytes).unwrap();
        }
        let patch = built(
            repo.path(),
            &[
                modified("one.txt", b"a\n", b"b\n"),
                modified("two.txt", b"c\n", b"d\n"),
            ],
        );
        let data = data.path();
        let bins = [binary(None, Some("x.png"))];
        let made = create(data, repo.path(), "mixed", &patch, &bins, false).unwrap();
        let loaded = load_patch(data, "mixed").unwrap();
        let second = Patch {
            files: vec![loaded.files[1].clone()],
            ..loaded.clone()
        };

        let copy = save_remaining_and_delete_others(data, "mixed", &second, &[], at(NOW))
            .unwrap()
            .unwrap();
        assert_eq!(copy.name, "mixed1");
        assert!(copy.deleted && !copy.recycled);
        assert_eq!(copy.date_ms, NOW);
        assert_eq!(load_patch(data, "mixed1").unwrap().files, loaded.files[..1]);
        assert_eq!(copy.binaries.len(), 1);
        let kept = shelf_of(data, "mixed");
        assert!(!kept.deleted && kept.binaries.is_empty());
        assert_eq!(kept.date_ms, made.date_ms);
        assert_eq!(load_patch(data, "mixed").unwrap(), second);

        // Deleted already: no copy, and with nothing left the list is erased.
        create(data, repo.path(), "again", &patch, &[], false).unwrap();
        delete(data, "again", at(NOW)).unwrap();
        let before = list(data).len();
        assert_eq!(
            save_remaining_and_delete_others(data, "again", &second, &[], at(NOW + 1)).unwrap(),
            None
        );
        assert_eq!(list(data).len(), before);
        assert!(shelf_of(data, "again").deleted);
        assert_eq!(shelf_of(data, "again").date_ms, NOW);
        assert_eq!(load_patch(data, "again").unwrap(), second);
        let none = Patch::default();
        assert_eq!(
            save_remaining_and_delete_others(data, "again", &none, &[], at(NOW)).unwrap(),
            None
        );
        assert_eq!(list(data).len(), before - 1);
        assert!(!data.join("shelf/again").exists());
    }

    #[test]
    fn restore_sets_the_date() {
        let data = TempDir::new().unwrap();
        let repo = TempDir::new().unwrap();
        let data = data.path();
        let made = create(data, repo.path(), "keep", &Patch::default(), &[], false).unwrap();
        delete(data, "keep", at(NOW)).unwrap();

        // Undo gives back the original date, the Restore action the moment it runs.
        let restored = restore(data, "keep", made.date_ms).unwrap();
        assert_eq!(restored, made);
        assert_eq!(shelf_of(data, "keep"), made);
        delete(data, "keep", at(NOW)).unwrap();
        assert_eq!(restore(data, "keep", NOW + 5).unwrap().date_ms, NOW + 5);

        // Rename changes the description only: the folder and the name stay.
        let renamed = rename(data, "keep", "New <text>").unwrap();
        assert_eq!(
            (renamed.name.as_str(), renamed.description.as_str()),
            ("keep", "New <text>")
        );
        assert_eq!(renamed.patch, made.patch);
        assert_eq!(shelf_of(data, "keep"), renamed);
    }

    #[test]
    fn cleanup_erases_deleted_older_than_7_days_and_turns_system_recycled_into_deleted() {
        let data = TempDir::new().unwrap();
        let repo = TempDir::new().unwrap();
        let data = data.path();
        for name in ["old", "week", "recent", "plain"] {
            create(data, repo.path(), name, &Patch::default(), &[], false).unwrap();
        }
        delete(data, "old", at(NOW - 7 * DAY - 1)).unwrap();
        delete(data, "week", at(NOW - 7 * DAY)).unwrap();
        delete(data, "recent", at(NOW - DAY)).unwrap();
        recycle(data, "plain", at(NOW - 30 * DAY)).unwrap();
        // A system list recycled by an older build, with `toDelete` still set.
        let mut system = create(data, repo.path(), "system", &Patch::default(), &[], true).unwrap();
        system.recycled = true;
        store(data, &system).unwrap();

        assert_eq!(cleanup(data, at(NOW)).unwrap(), 1);
        let state: Vec<_> = list(data)
            .into_iter()
            .map(|list| (list.name, list.recycled, list.to_delete, list.deleted))
            .collect();
        let row = |name: &str, recycled, to_delete, deleted| {
            (name.to_string(), recycled, to_delete, deleted)
        };
        // Newest first.
        assert_eq!(
            state,
            [
                row("system", true, false, true),
                row("recent", false, false, true),
                row("week", false, false, true),
                row("plain", true, false, false),
            ]
        );
        assert_eq!(shelf_of(data, "system").date_ms, system.date_ms);
        assert!(!data.join("shelf/old").exists() && !data.join("shelf/old.xml").exists());
        assert_eq!(cleanup(data, at(NOW)).unwrap(), 0);
    }

    // The fixture `shelvedChangeLists` as IntelliJ keeps it in a project's `.idea/shelf`.
    fn idea_project() -> TempDir {
        let project = TempDir::new().unwrap();
        let idea = project.path().join(".idea");
        copy_fixture(
            &fixtures().join("shelvedChangeLists"),
            &idea.join("shelf"),
            "$PROJECT_DIR$/.idea/shelf/",
        );
        fs::write(idea.join("workspace.xml"), NO_CHANGELISTS).unwrap();
        project
    }

    #[test]
    fn import_is_read_only_and_repeatable() {
        let project = idea_project();
        let idea = project.path().join(".idea");
        let before = snapshot(&idea);
        let data = TempDir::new().unwrap();
        let data = data.path();

        let result = import(data, project.path()).unwrap();
        assert_eq!(
            result,
            ImportResult {
                added: 3,
                skipped: 0
            }
        );
        let lists = list(data);
        let names: Vec<_> = lists.iter().map(|list| list.description.as_str()).collect();
        assert_eq!(names, ["description", "description1", "description2"]);
        for list in &lists {
            assert_eq!(list.name, list.description);
            assert_eq!(list.date_ms, 1539026452817);
            assert!(!list.recycled && !list.deleted && !list.to_delete);
            let folder = data.join("shelf").join(&list.name);
            assert_eq!(list.patch, folder.join("shelved.patch"));
            let source = match list.name.as_str() {
                "description" => "test",
                "description1" => "test1",
                _ => "test2",
            };
            let source = idea.join("shelf").join(source);
            assert_eq!(
                fs::read(&list.patch).unwrap(),
                fs::read(source.join("shelved.patch")).unwrap()
            );
            let after: Vec<_> = list.binaries.iter().map(|b| b.after.clone()).collect();
            assert_eq!(after, [some("c.png"), some("c1.png")]);
            for binary in &list.binaries {
                assert_eq!(binary.before, None);
                let shelved = binary.shelved.as_ref().unwrap();
                assert_eq!(shelved, &folder.join(binary.after.as_ref().unwrap()));
                let original = source.join(binary.after.as_ref().unwrap());
                assert_eq!(fs::read(shelved).unwrap(), fs::read(original).unwrap());
            }
        }

        // A second import adds nothing.
        let again = import(data, project.path()).unwrap();
        assert_eq!(
            again,
            ImportResult {
                added: 0,
                skipped: 3
            }
        );
        assert_eq!(list(data), lists);

        // A list deleted in rama, even erased, does not come back.
        delete(data, "description", at(NOW)).unwrap();
        delete(data, "description", at(NOW)).unwrap();
        assert_eq!(list(data).len(), 2);
        let third = import(data, project.path()).unwrap();
        assert_eq!(
            third,
            ImportResult {
                added: 0,
                skipped: 3
            }
        );
        assert_eq!(list(data).len(), 2);

        assert_eq!(snapshot(&idea), before);
    }

    // P8: two lists that differ only in their binary copies are two lists. A record written before
    // the copies were part of the key still keeps its list from coming back.
    #[test]
    fn lists_that_differ_only_in_their_binaries_are_both_imported() {
        let project = idea_project();
        let shelf = project.path().join(".idea/shelf");
        copy_fixture(&shelf.join("test"), &shelf.join("test3"), "");
        fs::write(shelf.join("test3/c.png"), [9, 9, 9]).unwrap();
        let xml = fs::read_to_string(shelf.join("test.xml"))
            .unwrap()
            .replace("name=\"test\"", "name=\"test3\"")
            .replace("/test/", "/test3/");
        fs::write(shelf.join("test3.xml"), xml).unwrap();
        let data = TempDir::new().unwrap();
        let data = data.path();

        assert_eq!(import(data, project.path()).unwrap().added, 4);
        assert_eq!(import(data, project.path()).unwrap().added, 0);

        let record = data.join("shelf").join(IMPORTED_FILE);
        let mut old: serde_json::Value =
            serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
        for key in old["lists"].as_array_mut().unwrap() {
            key.as_object_mut().unwrap().remove("binaries");
        }
        fs::write(&record, serde_json::to_vec(&old).unwrap()).unwrap();
        for name in [
            "description",
            "description1",
            "description2",
            "description3",
        ] {
            delete(data, name, at(NOW)).unwrap();
            delete(data, name, at(NOW)).unwrap();
        }
        assert_eq!(import(data, project.path()).unwrap().added, 0);
    }

    #[test]
    fn import_reads_a_custom_shelf_path() {
        let top = TempDir::new().unwrap();
        let project = top.path().join("project");
        let custom = top.path().join("custom");
        // A custom shelf keeps absolute paths (`shouldCollapsePath` is false).
        let prefix = format!("{}/", custom.to_str().unwrap());
        copy_fixture(&fixtures().join("shelvedChangeLists"), &custom, &prefix);
        fs::remove_file(custom.join("test1.xml")).unwrap();
        fs::remove_file(custom.join("test2.xml")).unwrap();
        // What the default place holds is not read while the custom one is on.
        let idea = project.join(".idea");
        copy_fixture(
            &fixtures().join("shelvedChangeLists"),
            &idea.join("shelf"),
            "$PROJECT_DIR$/.idea/shelf/",
        );
        let workspace = |on: &str| {
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<project version="4">
  <component name="VcsManagerConfiguration">
    <option name="USE_CUSTOM_SHELF_PATH" value="{on}" />
    <option name="CUSTOM_SHELF_PATH" value="$PROJECT_DIR$/../custom" />
  </component>
</project>"#
            )
        };
        fs::write(idea.join("workspace.xml"), workspace("true")).unwrap();
        let before = (snapshot(&idea), snapshot(&custom));

        let data = TempDir::new().unwrap();
        assert_eq!(
            import(data.path(), &project).unwrap(),
            ImportResult {
                added: 1,
                skipped: 0
            }
        );
        let lists = list(data.path());
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].description, "description");
        assert_eq!(
            fs::read(&lists[0].patch).unwrap(),
            fs::read(custom.join("test/shelved.patch")).unwrap()
        );
        assert_eq!((snapshot(&idea), snapshot(&custom)), before);

        // Off, the option leaves the shelf in `.idea/shelf`.
        fs::write(idea.join("workspace.xml"), workspace("false")).unwrap();
        let data = TempDir::new().unwrap();
        assert_eq!(
            import(data.path(), &project).unwrap(),
            ImportResult {
                added: 3,
                skipped: 0
            }
        );
    }

    // Names and paths from `.idea` and from the store's own XML are untrusted: a name never
    // reaches a path, and a patch or a binary outside its folder makes the list unreadable.
    #[test]
    fn untrusted_names_and_paths_stay_inside_the_shelf() {
        let project = TempDir::new().unwrap();
        let idea = project.path().join(".idea");
        let shelf = idea.join("shelf");
        fs::create_dir_all(&shelf).unwrap();
        fs::write(idea.join("workspace.xml"), NO_CHANGELISTS).unwrap();
        let patch_text = "--- a.txt\n+++ a.txt\n@@ -0,0 +1 @@\n+a\n";
        fs::write(project.path().join("secret.patch"), patch_text).unwrap();
        for folder in ["evil", "link", "bin"] {
            fs::create_dir(shelf.join(folder)).unwrap();
        }
        fs::write(shelf.join("evil/shelved.patch"), patch_text).unwrap();
        fs::write(shelf.join("bin/shelved.patch"), patch_text).unwrap();
        symlink("../../../secret.patch", shelf.join("link/shelved.patch")).unwrap();
        let in_shelf = "$PROJECT_DIR$/.idea/shelf";
        let xml = |name: &str, path: &str, binary: &str| {
            format!(
                "<changelist name=\"{name}\" date=\"1\" recycled=\"false\">\n  \
                 <option name=\"PATH\" value=\"{path}\" />\n  \
                 <option name=\"DESCRIPTION\" value=\"../{name}\" />\n{binary}</changelist>"
            )
        };
        let binary = |after: &str, shelved: &str| {
            format!(
                "  <binary>\n    <option name=\"AFTER_PATH\" value=\"{after}\" />\n    \
                 <option name=\"SHELVED_PATH\" value=\"{shelved}\" />\n  </binary>\n"
            )
        };
        let files = [
            (
                "evil.xml",
                xml(
                    "../../../evil",
                    &format!("{in_shelf}/evil/shelved.patch"),
                    "",
                ),
            ),
            (
                "outside.xml",
                xml("outside", "$PROJECT_DIR$/secret.patch", ""),
            ),
            (
                "dotdot.xml",
                xml("dotdot", &format!("{in_shelf}/../../secret.patch"), ""),
            ),
            (
                "link.xml",
                xml("link", &format!("{in_shelf}/link/shelved.patch"), ""),
            ),
            (
                "bin.xml",
                xml(
                    "bin",
                    &format!("{in_shelf}/bin/shelved.patch"),
                    &binary("c.png", &format!("{in_shelf}/evil/shelved.patch")),
                ),
            ),
            (
                "abs.xml",
                xml(
                    "abs",
                    &format!("{in_shelf}/bin/shelved.patch"),
                    &binary("/etc/hosts", &format!("{in_shelf}/bin/shelved.patch")),
                ),
            ),
            ("broken.xml", "<changelist name=\"x\"".to_string()),
        ];
        for (file, text) in &files {
            fs::write(shelf.join(file), text).unwrap();
        }
        let before = snapshot(&idea);
        let data = TempDir::new().unwrap();
        let data = data.path();

        assert_eq!(
            import(data, project.path()).unwrap(),
            ImportResult {
                added: 1,
                skipped: 6
            }
        );
        let lists = list(data);
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].name, "____________evil");
        assert_eq!(lists[0].description, "../../../../evil");
        let mut stored: Vec<_> = fs::read_dir(data.join("shelf"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        stored.sort();
        assert_eq!(
            stored,
            ["____________evil", "____________evil.xml", "imported.json"]
        );
        let top: Vec<_> = fs::read_dir(data)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(top, ["shelf"]);
        assert_eq!(snapshot(&idea), before);

        // The store's own XML: a name that is not the file's, or a patch outside the store, and
        // the list is not there.
        let root = data.join("shelf");
        fs::write(
            root.join("x.xml"),
            xml("../y", &format!("{}/x/shelved.patch", root.display()), ""),
        )
        .unwrap();
        fs::write(root.join("z.xml"), xml("z", "/etc/hosts", "")).unwrap();
        assert_eq!(list(data).len(), 1);
        for name in ["../x", "/etc/passwd", "a/b", "..", "", ".hidden", "z"] {
            assert!(load_patch(data, name).is_err(), "{name}");
            assert!(delete(data, name, at(NOW)).is_err(), "{name}");
            assert!(rename(data, name, "x").is_err(), "{name}");
            assert!(restore(data, name, 0).is_err(), "{name}");
            assert!(recycle(data, name, at(NOW)).is_err(), "{name}");
        }
        assert!(Path::new("/etc/hosts").is_file());
    }

    // Review Focus 2: a shelf written by rama reads back identical.
    #[test]
    fn round_trip_with_binary_rename_delete_no_newline_crlf_non_utf8() {
        let repo = TempRepo::new();
        let committed: [(&str, &[u8]); 6] = [
            ("old.txt", b"a\nb\nc\n"),
            ("gone.txt", b"x\n"),
            ("tail.txt", b"1\n2"),
            ("crlf.txt", b"a\r\nb\r\n"),
            ("latin.txt", b"caf\xe9\n"),
            ("img.png", &[0, 159, 146, 150, 0, 1]),
        ];
        for (path, bytes) in committed {
            fs::write(repo.path().join(path), bytes).unwrap();
        }
        repo.git(&["add", "-A"]);
        repo.git(&["commit", "-q", "-m", "base"]);
        let head = Some(repo.git(&["rev-parse", "HEAD"]).trim().to_string());
        fs::remove_file(repo.path().join("old.txt")).unwrap();
        fs::remove_file(repo.path().join("gone.txt")).unwrap();
        let working: [(&str, &[u8]); 5] = [
            ("new.txt", b"a\nB\nc\n"),
            ("tail.txt", b"1\n3"),
            ("crlf.txt", b"a\r\nc\r\n"),
            ("latin.txt", b"caf\xe9!\n"),
            ("img.png", &[0, 159, 146, 150, 0, 2]),
        ];
        for (path, bytes) in working {
            fs::write(repo.path().join(path), bytes).unwrap();
        }
        let mut entries = vec![
            PatchSource {
                before: side("old.txt", b"a\nb\nc\n"),
                after: side("new.txt", b"a\nB\nc\n"),
                before_rev: head.clone(),
                binary: false,
            },
            PatchSource {
                before: side("gone.txt", b"x\n"),
                before_rev: head.clone(),
                ..PatchSource::default()
            },
        ];
        for (path, before, after) in [
            ("tail.txt", &b"1\n2"[..], &b"1\n3"[..]),
            ("crlf.txt", b"a\r\nb\r\n", b"a\r\nc\r\n"),
            ("latin.txt", b"caf\xe9\n", b"caf\xe9!\n"),
        ] {
            entries.push(PatchSource {
                before_rev: head.clone(),
                ..modified(path, before, after)
            });
        }
        let patch = built(repo.path(), &entries);
        assert_eq!(patch.files.len(), 5);
        let data = TempDir::new().unwrap();
        let data = data.path();
        let bins = [binary(Some("img.png"), Some("img.png"))];
        let made = create(data, repo.path(), "Round trip", &patch, &bins, false).unwrap();

        assert_eq!(
            load_patch(data, &made.name).unwrap(),
            without_versions(patch.clone())
        );
        assert_eq!(list(data), std::slice::from_ref(&made));
        let copy = made.binaries[0].shelved.as_ref().unwrap();
        assert_eq!(fs::read(copy).unwrap(), [0, 159, 146, 150, 0, 2]);
        // Bases are LF, and only for modified UTF-8 files.
        let base = |path: &str| {
            patch
                .files
                .iter()
                .map(text)
                .find(|file| file.after.as_deref() == Some(path))
                .unwrap()
                .base
                .clone()
        };
        assert_eq!(base("crlf.txt"), Some(b"a\nb\n".to_vec()));
        assert_eq!(base("tail.txt"), Some(b"1\n2".to_vec()));
        assert_eq!(base("latin.txt"), None);

        // Rewritten when part of it is applied, the rest still reads back identical.
        let remaining = Patch {
            files: patch.files[1..].to_vec(),
            ..patch.clone()
        };
        save_remaining_and_recycle_applied(data, &made.name, &remaining, &made.binaries, at(NOW))
            .unwrap()
            .unwrap();
        assert_eq!(
            load_patch(data, &made.name).unwrap(),
            without_versions(remaining)
        );
        let applied = load_patch(data, "Round_trip1").unwrap();
        assert_eq!(applied.files, without_versions(patch).files[..1]);
    }
}
