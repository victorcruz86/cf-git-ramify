// The read-only viewer: a file on disk or at a revision, with IntelliJ's size and binary rules, and
// the window that shows it, one tab per file (decision 2 of phase 4 part 4).

use std::env;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::PoisonError;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::commands::{
    self, blocking_api, current_repo, failed, owner_label, reveal, session_of, try_emit_to_owner,
    ApiError, AppState, WindowSession,
};
use crate::diff::check_hash;
use crate::git::{self, GitError};
use crate::windows::{self, WindowKind};
use crate::worktree_diff;

const VIEWER_WINDOW_URL: &str = "index.html?window=viewer";
// The page takes what is waiting with `viewer_targets`: on mounting, and on each of these.
const VIEWER_OPEN_EVENT: &str = "viewer-open";

const KILOBYTE: u64 = 1024;
const MEGABYTE: u64 = KILOBYTE * KILOBYTE;
// `FileUtilRt.LARGE_FOR_CONTENT_LOADING` = max(20 * MEGABYTE, `idea.max.intellisense.filesize`,
// `idea.max.content.load.filesize`), which the installed `bin/idea.properties` sets to 2500 and
// 20,000 KB: 20 MiB. The page gets what it shows from `ViewerContent`, never a copy of these.
pub const CONTENT_LOAD_LIMIT: u64 = 20 * MEGABYTE;
// `FileUtilRt.LARGE_FILE_PREVIEW_SIZE`: `idea.max.content.load.large.preview.size`, 2500 KB by default.
pub const PREVIEW_SIZE: u64 = 2_500 * KILOBYTE;
// `CharsetToolkit.BINARY_THRESHOLD`: a byte below it is a control character no text has.
const BINARY_THRESHOLD: u8 = 9;

// The types IntelliJ CE registers as binary (images, archives, class files): binary whatever their bytes.
const BINARY_EXTENSIONS: &str = "png jpg jpeg gif bmp ico wbmp webp tif tiff \
    zip jar war ear swc ane egg apk aar class";
// Text types with a language: text whatever their bytes, in the default charset (UTF-8).
const TEXT_EXTENSIONS: &str = "txt md markdown java kt kts groovy gradle scala xml html htm xhtml \
    svg css scss sass less js mjs cjs jsx ts mts cts tsx json json5 yaml yml toml ini cfg conf \
    properties sh bash zsh fish bat cmd ps1 py rb go rs c h cc cpp cxx hpp hh m mm swift cs fs php \
    pl lua r sql svelte vue dart ex exs erl hs clj elm tf proto graphql csv tsv log diff patch";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewerTarget {
    // Relative to the repository root, with `/`.
    pub path: String,
    // A full hash; without it, the file on disk.
    pub revision: Option<String>,
    // From 1.
    pub line: Option<u32>,
    // `Annotate` from a menu outside the viewer: the tab shows its annotations once open.
    #[serde(default)]
    pub annotate: bool,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ViewerContent {
    Text {
        text: String,
        // The size of the whole file when `text` is only its first `preview_size` bytes.
        #[serde(rename = "previewOf")]
        preview_of: Option<u64>,
        #[serde(rename = "previewSize")]
        preview_size: Option<u64>,
    },
    Binary {
        size: u64,
        // Over `CONTENT_LOAD_LIMIT`: `binary.file.too.large` (`IB:1968`).
        #[serde(rename = "tooLarge")]
        too_large: bool,
    },
    Absent,
}

// `message.text.could.not.load.virtual.file.content` (`V:98`), with the full path as `getPresentableUrl`.
#[derive(Debug, PartialEq, Eq)]
pub struct LoadError {
    pub file: String,
    pub reason: String,
}

impl fmt::Display for LoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Could not load content for file {}: {}",
            self.file, self.reason
        )
    }
}

// What the interface shows of a failure: git's own words, as `VcsException.getLocalizedMessage`.
fn reason(error: GitError) -> String {
    match error {
        GitError::Failed { stderr, .. } => stderr.trim().to_string(),
        other => ApiError::from(other).message,
    }
}

fn io_reason(error: io::Error) -> String {
    error.to_string()
}

pub fn read(repo: &Path, target: &ViewerTarget) -> Result<ViewerContent, LoadError> {
    match &target.revision {
        None => read_disk(repo, &target.path),
        Some(revision) => read_revision(repo, revision, &target.path),
    }
    .map_err(|reason| LoadError {
        file: repo.join(&target.path).display().to_string(),
        reason,
    })
}

// The local file as the disk has it, following its link inside the repository.
fn read_disk(repo: &Path, path: &str) -> Result<ViewerContent, String> {
    worktree_diff::check_inside(repo, path).map_err(|error| ApiError::from(error).message)?;
    let full = repo.join(path);
    let meta = match fs::metadata(&full) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(ViewerContent::Absent),
        Err(error) => return Err(io_reason(error)),
    };
    if !meta.is_file() {
        return Ok(ViewerContent::Absent);
    }
    if meta.len() > CONTENT_LOAD_LIMIT {
        let mut prefix = Vec::new();
        File::open(&full)
            .and_then(|file| file.take(PREVIEW_SIZE).read_to_end(&mut prefix))
            .map_err(io_reason)?;
        return Ok(preview(path, &prefix, meta.len()));
    }
    Ok(whole(path, fs::read(&full).map_err(io_reason)?))
}

// The entry at `<revision> -- <path>` with the exact bytes of the name (part 3's declined item): with
// `core.precomposeunicode` git would look up the composed name instead. Its blob is then read by id.
fn read_revision(repo: &Path, revision: &str, path: &str) -> Result<ViewerContent, String> {
    check_hash(revision).map_err(reason)?;
    // A revision that does not exist would read as a missing path: it is checked on its own.
    let commit = format!("{revision}^{{commit}}");
    git::run(repo, &["cat-file", "-e", &commit]).map_err(reason)?;
    let args = git::exact(&[
        "--literal-pathspecs",
        "ls-tree",
        "-l",
        "-z",
        revision,
        "--",
        path,
    ]);
    let listed = git::run(repo, &args).map_err(reason)?;
    let Some((blob, size)) = blob_entry(&listed, path) else {
        return Ok(ViewerContent::Absent);
    };
    if size > CONTENT_LOAD_LIMIT {
        return Ok(preview(path, &blob_prefix(repo, &blob)?, size));
    }
    let bytes = git::run(repo, &["cat-file", "blob", &blob]).map_err(reason)?;
    Ok(whole(path, bytes))
}

// `<mode> <type> <object> <size>\t<path>` records: only a blob at exactly `path` has a text; a
// folder is a `tree` and a submodule a `commit`.
fn blob_entry(listed: &[u8], path: &str) -> Option<(String, u64)> {
    listed.split(|byte| *byte == 0).find_map(|record| {
        let tab = record.iter().position(|byte| *byte == b'\t')?;
        if &record[tab + 1..] != path.as_bytes() {
            return None;
        }
        let head = String::from_utf8_lossy(&record[..tab]);
        match head.split_whitespace().collect::<Vec<_>>().as_slice() {
            [_, "blob", object, size] => Some((object.to_string(), size.parse().ok()?)),
            _ => None,
        }
    })
}

// The first `PREVIEW_SIZE` bytes, without reading the rest: git is stopped once they arrive.
fn blob_prefix(repo: &Path, object: &str) -> Result<Vec<u8>, String> {
    let binary = git::git_binary();
    let mut child = git::command(&binary, repo, &["cat-file", "blob", object])
        .map_err(reason)?
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| reason(git::spawn_error(&binary, error)))?;
    let mut prefix = Vec::new();
    let read = child
        .stdout
        .take()
        .expect("stdout with pipe")
        .take(PREVIEW_SIZE)
        .read_to_end(&mut prefix);
    let _ = child.kill();
    let _ = child.wait();
    read.map_err(io_reason)?;
    Ok(prefix)
}

fn whole(path: &str, bytes: Vec<u8>) -> ViewerContent {
    if is_binary(path, &bytes) {
        return ViewerContent::Binary {
            size: bytes.len() as u64,
            too_large: false,
        };
    }
    ViewerContent::Text {
        text: decode(&bytes),
        preview_of: None,
        preview_size: None,
    }
}

// `LargeFileEditorProvider`: a read-only preview of the start; the cut does not split a character.
fn preview(path: &str, prefix: &[u8], size: u64) -> ViewerContent {
    let prefix = whole_characters(prefix);
    if is_binary(path, prefix) {
        return ViewerContent::Binary {
            size,
            too_large: true,
        };
    }
    ViewerContent::Text {
        text: decode(prefix),
        preview_of: Some(size),
        preview_size: Some(PREVIEW_SIZE),
    }
}

fn whole_characters(bytes: &[u8]) -> &[u8] {
    match std::str::from_utf8(bytes) {
        Err(error) if error.error_len().is_none() => &bytes[..error.valid_up_to()],
        _ => bytes,
    }
}

#[derive(Debug, PartialEq, Eq)]
enum FileType {
    Binary,
    Text,
    Unknown,
}

fn file_type(path: &str) -> FileType {
    let name = path.rsplit('/').next().unwrap_or(path);
    let Some((_, extension)) = name.rsplit_once('.') else {
        return FileType::Unknown;
    };
    let extension = extension.to_ascii_lowercase();
    let listed = |list: &str| list.split_whitespace().any(|known| known == extension);
    if listed(BINARY_EXTENSIONS) {
        FileType::Binary
    } else if listed(TEXT_EXTENSIONS) {
        FileType::Text
    } else {
        FileType::Unknown
    }
}

// `FileType.isBinary` of a file by its name alone: the file menus ask it before anything is read.
pub fn binary_type(path: &str) -> bool {
    file_type(path) == FileType::Binary
}

// `DiffContentFactoryImpl.isBinaryContent`. An empty file of an unknown type is shown as an empty
// text: IntelliJ's editor detects its type from the content, and the diff's rule would call it binary.
fn is_binary(path: &str, bytes: &[u8]) -> bool {
    match file_type(path) {
        FileType::Binary => true,
        FileType::Text => false,
        FileType::Unknown => !bytes.is_empty() && guess_charset(bytes).is_none(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Charset {
    Utf8Bom,
    Utf32Be,
    Utf32Le,
    Utf16Le,
    Utf16Be,
    Ascii,
    Utf8,
}

// `CharsetToolkit.guessFromBOM`, in its order.
fn charset_from_bom(bytes: &[u8]) -> Option<Charset> {
    [
        (&b"\xef\xbb\xbf"[..], Charset::Utf8Bom),
        (b"\0\0\xfe\xff", Charset::Utf32Be),
        (b"\xff\xfe\0\0", Charset::Utf32Le),
        (b"\xff\xfe", Charset::Utf16Le),
        (b"\xfe\xff", Charset::Utf16Be),
    ]
    .into_iter()
    .find(|(bom, _)| bytes.starts_with(bom))
    .map(|(_, charset)| charset)
}

// `DiffContentFactoryImpl.guessCharsetFromContent`.
fn guess_charset(bytes: &[u8]) -> Option<Charset> {
    charset_from_bom(bytes).or(match guess_from_content(bytes) {
        Guessed::SevenBit => Some(Charset::Ascii),
        Guessed::ValidUtf8 => Some(Charset::Utf8),
        Guessed::InvalidUtf8 | Guessed::Binary => None,
    })
}

#[derive(Debug, PartialEq, Eq)]
enum Guessed {
    SevenBit,
    ValidUtf8,
    InvalidUtf8,
    Binary,
}

// `CharsetToolkit.guessFromContent`, over the whole content. Its signed byte ranges are written
// unsigned: a continuation byte is 0x80-0xBF, a lead byte of 2 to 6 bytes 0xC0-0xFD.
fn guess_from_content(bytes: &[u8]) -> Guessed {
    let continuation = |at: usize| {
        bytes
            .get(at)
            .is_some_and(|byte| (0x80..=0xbf).contains(byte))
    };
    let mut high_order_bit = false;
    let mut valid_utf8 = true;
    let mut has_binary = false;
    let mut at = 0;
    while at < bytes.len() {
        let lead = bytes[at];
        if lead >= 0x80 {
            high_order_bit = true;
            let followers = match lead {
                0xc0..=0xdf => 1,
                0xe0..=0xef => 2,
                0xf0..=0xf7 => 3,
                0xf8..=0xfb => 4,
                0xfc..=0xfd => 5,
                _ => {
                    valid_utf8 = false;
                    0
                }
            };
            if valid_utf8 {
                if (1..=followers).all(|offset| continuation(at + offset)) {
                    at += followers;
                } else {
                    valid_utf8 = false;
                }
            }
        } else if lead < BINARY_THRESHOLD {
            has_binary = true;
            break;
        }
        if !valid_utf8 {
            break;
        }
        at += 1;
    }
    if !high_order_bit && !has_binary {
        Guessed::SevenBit
    } else if !valid_utf8 {
        Guessed::InvalidUtf8
    } else if has_binary {
        Guessed::Binary
    } else {
        Guessed::ValidUtf8
    }
}

// The text without its BOM (`VfsUtilCore.byteStreamSkippingBOM`), in the charset the BOM names; without
// one, UTF-8, the default.
fn decode(bytes: &[u8]) -> String {
    let units = |rest: &[u8], size: usize, from: fn(&[u8]) -> u32| -> Vec<u32> {
        rest.chunks_exact(size).map(from).collect()
    };
    match charset_from_bom(bytes) {
        Some(Charset::Utf8Bom) => String::from_utf8_lossy(&bytes[3..]).into_owned(),
        Some(Charset::Utf16Le) => utf16(&units(&bytes[2..], 2, |pair| {
            u32::from(u16::from_le_bytes([pair[0], pair[1]]))
        })),
        Some(Charset::Utf16Be) => utf16(&units(&bytes[2..], 2, |pair| {
            u32::from(u16::from_be_bytes([pair[0], pair[1]]))
        })),
        Some(Charset::Utf32Le) => utf32(&units(&bytes[4..], 4, |four| {
            u32::from_le_bytes([four[0], four[1], four[2], four[3]])
        })),
        Some(Charset::Utf32Be) => utf32(&units(&bytes[4..], 4, |four| {
            u32::from_be_bytes([four[0], four[1], four[2], four[3]])
        })),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

fn utf16(units: &[u32]) -> String {
    let units: Vec<u16> = units.iter().map(|unit| *unit as u16).collect();
    String::from_utf16_lossy(&units)
}

fn utf32(units: &[u32]) -> String {
    units
        .iter()
        .map(|unit| char::from_u32(*unit).unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

// What tells a later `repo-changed` whether the local file is still the one read: size, inode and
// modification time, without reading it. `None` when it is not a file inside the repository.
pub fn stamp(repo: &Path, path: &str) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    worktree_diff::check_inside(repo, path).ok()?;
    let meta = fs::metadata(repo.join(path)).ok()?;
    if !meta.is_file() {
        return None;
    }
    Some(format!(
        "{}:{}:{}.{}",
        meta.len(),
        meta.ino(),
        meta.mtime(),
        meta.mtime_nsec()
    ))
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct InitialTarget {
    pub repo: String,
    // Relative to `repo`, with `/`.
    pub file: Option<String>,
    pub line: Option<u32>,
}

// Decision 1: `Ramify <file>` and `Ramify <file>:<line>` open the file's repository and the viewer at
// that line. Anything else (a folder, or what does not exist) goes to `repo_open` as it is.
pub fn initial_target_of(argument: Option<String>) -> Result<Option<InitialTarget>, ApiError> {
    let Some(argument) = argument else {
        return Ok(None);
    };
    let (file, line) = if Path::new(&argument).is_file() {
        (argument.as_str(), None)
    } else {
        match split_line(&argument) {
            Some((file, line)) if Path::new(file).is_file() => (file, Some(line)),
            _ => {
                return Ok(Some(InitialTarget {
                    repo: argument,
                    file: None,
                    line: None,
                }))
            }
        }
    };
    let (repo, relative) = locate(Path::new(file)).map_err(|error| match error {
        GitError::NotARepository { .. } => ApiError {
            kind: "not_a_repository",
            message: format!("{file} is not inside a Git repository"),
            details: None,
        },
        other => other.into(),
    })?;
    Ok(Some(InitialTarget {
        repo: repo.display().to_string(),
        file: Some(relative),
        line,
    }))
}

fn split_line(argument: &str) -> Option<(&str, u32)> {
    let (file, line) = argument.rsplit_once(':')?;
    let line = line.parse().ok().filter(|line| *line >= 1)?;
    Some((file, line))
}

// The root from the file's folder, as `repo_root` does from a folder; the path keeps the file's own
// name, so a link is not followed.
fn locate(file: &Path) -> Result<(PathBuf, String), GitError> {
    let io = |error: io::Error| GitError::Io(error.to_string());
    let absolute = std::path::absolute(file).map_err(io)?;
    let (Some(folder), Some(name)) = (absolute.parent(), absolute.file_name()) else {
        return Err(GitError::Io(format!("{} is not a file", file.display())));
    };
    let folder = fs::canonicalize(folder).map_err(io)?;
    let root = fs::canonicalize(commands::repo_root(&folder)?).map_err(io)?;
    let inside = folder
        .strip_prefix(&root)
        .map_err(|_| GitError::Io(format!("{} is outside {}", file.display(), root.display())))?;
    let mut parts: Vec<String> = inside
        .components()
        .filter_map(|part| match part {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    parts.push(name.to_string_lossy().into_owned());
    Ok((root, parts.join("/")))
}

// Per main window: what the open flow stored for it wins, and only the first `main` reads the
// command line. Without a path there, the first time `main` asks, it reopens the first project
// of the open list (`commands::reopen_first`); a start with a path does not read the list.
#[tauri::command]
pub fn initial_target(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Option<InitialTarget>, ApiError> {
    let argument = {
        let mut targets = state.initial_targets.lock().unwrap();
        let first_ask = !targets.asked(window.label());
        targets.target_for(window.label(), || {
            commands::first_argument(env::args_os()).or_else(|| {
                if first_ask {
                    commands::reopen_first(&app)
                } else {
                    None
                }
            })
        })
    };
    initial_target_of(argument)
}

// `ChangesUtil.iterateFiles`: a local file that is not on disk is skipped (Edit Source of a
// deletion opens nothing). Every path stays inside the repository.
fn openable(repo: &Path, targets: Vec<ViewerTarget>) -> Result<Vec<ViewerTarget>, ApiError> {
    let mut kept = Vec::new();
    for target in targets {
        match &target.revision {
            Some(revision) => check_hash(revision)?,
            None => {
                worktree_diff::check_inside(repo, &target.path)?;
                if !repo.join(&target.path).is_file() {
                    continue;
                }
            }
        }
        kept.push(target);
    }
    Ok(kept)
}

fn pending(session: &WindowSession) -> std::sync::MutexGuard<'_, Vec<ViewerTarget>> {
    session
        .viewer_pending
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

// One viewer window, reused, with a tab per file. Like the diff window, it is created hidden and its
// page shows it once mounted; the targets wait here until the page takes them.
#[tauri::command]
pub async fn viewer_open(
    targets: Vec<ViewerTarget>,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    let repo = current_repo(&state, &window)?;
    let title = repo
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let targets = blocking_api(move || openable(&repo, targets)).await?;
    if targets.is_empty() {
        return Ok(());
    }
    let session = session_of(&state, window.label())?;
    pending(&session).extend(targets);
    let owner = owner_label(&window);
    let label = windows::label_for(WindowKind::Viewer, &owner);
    let opened = match app.get_webview_window(&label) {
        Some(viewer) => {
            reveal(&viewer).and_then(|()| try_emit_to_owner(&app, &owner, VIEWER_OPEN_EVENT, ()))
        }
        None => WebviewWindowBuilder::new(&app, &label, WebviewUrl::App(VIEWER_WINDOW_URL.into()))
            .title(title)
            .inner_size(1000.0, 800.0)
            .visible(false)
            .background_color(tauri::webview::Color(0x1e, 0x1f, 0x22, 0xff))
            .build()
            .map(drop),
    };
    opened.map_err(|error| failed(error.to_string()))
}

// Which of the targets `viewer_open` would open: Edit Source is disabled when none.
#[tauri::command]
pub async fn viewer_openable(
    targets: Vec<ViewerTarget>,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Vec<ViewerTarget>, ApiError> {
    let repo = current_repo(&state, &window)?;
    blocking_api(move || openable(&repo, targets)).await
}

#[tauri::command]
pub async fn viewer_stamp(
    path: String,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Option<String>, ApiError> {
    let repo = current_repo(&state, &window)?;
    blocking_api(move || Ok(stamp(&repo, &path))).await
}

#[tauri::command]
pub fn viewer_targets(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Vec<ViewerTarget>, ApiError> {
    let session = session_of(&state, window.label())?;
    let targets = std::mem::take(&mut *pending(&session));
    Ok(targets)
}

#[tauri::command]
pub async fn viewer_content(
    path: String,
    revision: Option<String>,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<ViewerContent, ApiError> {
    let repo = current_repo(&state, &window)?;
    let target = ViewerTarget {
        path,
        revision,
        line: None,
        annotate: false,
    };
    blocking_api(move || read(&repo, &target).map_err(|error| failed(error.to_string()))).await
}

// Closing it, or opening another repository: its tabs go with it.
pub fn close(app: &AppHandle, owner: &str) {
    if let Ok(session) = session_of(&app.state::<AppState>(), owner) {
        release(&session);
    }
    if let Some(window) = app.get_webview_window(&windows::label_for(WindowKind::Viewer, owner)) {
        let _ = window.destroy();
    }
}

// What the window leaves behind: no target waiting.
pub fn release(session: &WindowSession) {
    pending(session).clear();
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::test_support::TempRepo;

    const NFC_NAME: &str = "caf\u{e9}.txt";
    const NFD_NAME: &str = "cafe\u{301}.txt";
    const PNG_HEADER: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0";

    fn local(path: &str) -> ViewerTarget {
        ViewerTarget {
            path: path.to_string(),
            revision: None,
            line: None,
            annotate: false,
        }
    }

    fn at(path: &str, revision: &str) -> ViewerTarget {
        ViewerTarget {
            revision: Some(revision.to_string()),
            ..local(path)
        }
    }

    fn text(content: ViewerContent) -> (String, Option<u64>) {
        match content {
            ViewerContent::Text {
                text, preview_of, ..
            } => (text, preview_of),
            other => panic!("expected text: {other:?}"),
        }
    }

    // A blob straight into the index under its exact name, as a clone from Linux leaves it.
    fn stage_exact(repo: &TempRepo, name: &str, content: &str) {
        let blob = crate::test_support::git_command()
            .current_dir(repo.path())
            .args(["hash-object", "-w", "--stdin"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child.stdin.take().unwrap().write_all(content.as_bytes())?;
                child.wait_with_output()
            })
            .unwrap();
        let blob = String::from_utf8(blob.stdout).unwrap();
        let info = format!("100644,{},{name}", blob.trim());
        repo.git(&[
            "-c",
            "core.precomposeunicode=false",
            "update-index",
            "--add",
            "--cacheinfo",
            &info,
        ]);
    }

    #[test]
    fn a_revision_is_read_at_its_own_path_through_exact() {
        let repo = TempRepo::new();
        stage_exact(&repo, NFC_NAME, "composed\n");
        stage_exact(&repo, NFD_NAME, "decomposed\n");
        repo.git(&["commit", "-q", "-m", "both forms"]);
        let head = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
        repo.git(&["config", "core.precomposeunicode", "true"]);

        let decomposed = read(repo.path(), &at(NFD_NAME, &head)).unwrap();
        let composed = read(repo.path(), &at(NFC_NAME, &head)).unwrap();

        assert_eq!(text(decomposed), ("decomposed\n".to_string(), None));
        assert_eq!(text(composed), ("composed\n".to_string(), None));
    }

    #[test]
    fn a_local_file_is_read_from_disk() {
        let repo = TempRepo::new();
        repo.commit("dir/a.txt", b"committed\n", "one");
        fs::write(repo.path().join("dir/a.txt"), "on disk\r\n").unwrap();

        let content = read(repo.path(), &local("dir/a.txt")).unwrap();

        assert_eq!(text(content), ("on disk\r\n".to_string(), None));
    }

    #[test]
    fn thirty_megabytes_give_a_preview_with_the_241_text() {
        let repo = TempRepo::new();
        let size = 30 * 1024 * 1024;
        let line = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcde\n";
        let content = line.repeat(size / line.len());
        let head = repo.commit("big.txt", content.as_bytes(), "big");

        for target in [local("big.txt"), at("big.txt", &head)] {
            let (shown, preview_of) = text(read(repo.path(), &target).unwrap());

            assert_eq!(preview_of, Some(content.len() as u64), "{target:?}");
            assert_eq!(shown.len() as u64, PREVIEW_SIZE, "{target:?}");
            // The page says how much it shows from what Rust says, not from a copy of the limit.
            assert!(
                matches!(read(repo.path(), &target).unwrap(), ViewerContent::Text { preview_size: Some(size), .. } if size == PREVIEW_SIZE),
                "{target:?}"
            );
            assert!(content.starts_with(&shown), "{target:?}");
        }
    }

    #[test]
    fn a_preview_does_not_cut_a_character_in_half() {
        let repo = TempRepo::new();
        // `é` is two bytes: 2,560,000 is even, so a leading `a` leaves the last one cut.
        let content = format!("a{}", "\u{e9}".repeat(11 * 1024 * 1024));
        fs::write(repo.path().join("accents.txt"), &content).unwrap();

        let (shown, preview_of) = text(read(repo.path(), &local("accents.txt")).unwrap());

        assert_eq!(preview_of, Some(content.len() as u64));
        assert_eq!(shown.len() as u64, PREVIEW_SIZE - 1);
        assert!(!shown.contains('\u{fffd}'));
    }

    // `FileUtilRt.LARGE_FOR_CONTENT_LOADING` = max(20 * MEGABYTE, the two user limits) = 20 MiB.
    #[test]
    fn the_limit_is_twenty_mebibytes() {
        let repo = TempRepo::new();
        let limit = 20 * 1024 * 1024;
        fs::write(repo.path().join("at.txt"), "x".repeat(limit)).unwrap();
        fs::write(repo.path().join("over.txt"), "x".repeat(limit + 1)).unwrap();

        let (shown, preview_of) = text(read(repo.path(), &local("at.txt")).unwrap());
        assert_eq!((shown.len(), preview_of), (limit, None));
        let (_, preview_of) = text(read(repo.path(), &local("over.txt")).unwrap());
        assert_eq!(preview_of, Some(limit as u64 + 1));
    }

    #[test]
    fn a_binary_over_the_limit_says_it_is_too_large() {
        let repo = TempRepo::new();
        let size = 20 * 1024 * 1024 + 1;
        fs::write(repo.path().join("huge.png"), vec![0u8; size]).unwrap();

        assert_eq!(
            read(repo.path(), &local("huge.png")).unwrap(),
            ViewerContent::Binary {
                size: size as u64,
                too_large: true
            }
        );
    }

    // `ls-tree` says what the path is: a folder or a submodule has no text to show.
    #[test]
    fn a_folder_or_a_submodule_at_a_revision_is_absent() {
        let repo = TempRepo::new();
        repo.commit("dir/a.txt", b"1\n", "one");
        let link = format!("160000,{},sub", "b".repeat(40));
        repo.git(&["update-index", "--add", "--cacheinfo", &link]);
        repo.git(&["commit", "-q", "-m", "submodule"]);
        let head = repo.git(&["rev-parse", "HEAD"]).trim().to_string();

        for path in ["dir", "sub"] {
            assert_eq!(
                read(repo.path(), &at(path, &head)).unwrap(),
                ViewerContent::Absent,
                "{path}"
            );
        }
        assert_eq!(
            text_of(&read(repo.path(), &at("dir/a.txt", &head)).unwrap()),
            "1\n"
        );
    }

    // What `repo-changed` compares before reading a local file again.
    #[test]
    fn the_stamp_of_a_local_file_follows_its_changes() {
        let repo = TempRepo::new();
        fs::write(repo.path().join("a.txt"), "1\n").unwrap();

        let first = stamp(repo.path(), "a.txt");
        assert!(first.is_some());
        assert_eq!(stamp(repo.path(), "a.txt"), first);
        fs::write(repo.path().join("a.txt"), "22\n").unwrap();
        assert_ne!(stamp(repo.path(), "a.txt"), first);
        fs::remove_file(repo.path().join("a.txt")).unwrap();
        assert_eq!(stamp(repo.path(), "a.txt"), None);
        assert_eq!(stamp(repo.path(), "../a.txt"), None);
    }

    #[test]
    fn a_png_is_binary() {
        let repo = TempRepo::new();
        let head = repo.commit("logo.png", PNG_HEADER, "logo");

        for target in [local("logo.png"), at("logo.png", &head)] {
            assert_eq!(
                read(repo.path(), &target).unwrap(),
                ViewerContent::Binary {
                    size: PNG_HEADER.len() as u64,
                    too_large: false
                },
                "{target:?}"
            );
        }
    }

    // `DiffContentFactoryImpl.isBinaryContent`: a known binary type is binary whatever its bytes, a
    // known text type is text, and an unknown one is binary when no charset can be guessed.
    #[test]
    fn binary_follows_the_file_type_then_the_content() {
        let repo = TempRepo::new();
        let cases: [(&str, &[u8], bool); 8] = [
            ("plain.png", b"only text", true),
            ("latin1.txt", b"caf\xe9\n", false),
            ("nul.java", b"a\0b", false),
            ("data.bin", b"a\0b", true),
            ("latin1.unknown", b"caf\xe9\n", true),
            ("utf8.unknown", "caf\u{e9}\n".as_bytes(), false),
            ("ascii", b"#!/bin/sh\n", false),
            ("utf16.unknown", b"\xff\xfeh\0i\0", false),
        ];
        for (name, bytes, _) in cases {
            fs::write(repo.path().join(name), bytes).unwrap();
        }

        for (name, bytes, binary) in cases {
            match read(repo.path(), &local(name)).unwrap() {
                ViewerContent::Binary { size, .. } => {
                    assert!(binary, "{name}");
                    assert_eq!(size, bytes.len() as u64, "{name}");
                }
                ViewerContent::Text { .. } => assert!(!binary, "{name}"),
                other => panic!("{name}: {other:?}"),
            }
        }
    }

    fn text_of(content: &ViewerContent) -> String {
        match content {
            ViewerContent::Text { text, .. } => text.clone(),
            other => panic!("expected text: {other:?}"),
        }
    }

    #[test]
    fn a_utf16_file_with_a_bom_is_decoded() {
        let repo = TempRepo::new();
        fs::write(repo.path().join("wide.txt"), b"\xff\xfeh\0i\0\n\0").unwrap();
        fs::write(repo.path().join("bom.txt"), b"\xef\xbb\xbfhi\n").unwrap();

        assert_eq!(
            text_of(&read(repo.path(), &local("wide.txt")).unwrap()),
            "hi\n"
        );
        assert_eq!(
            text_of(&read(repo.path(), &local("bom.txt")).unwrap()),
            "hi\n"
        );
    }

    // `AbstractVcsVirtualFile.showLoadingContentFailedMessage` (`V:98-99`).
    #[test]
    fn a_missing_revision_says_could_not_load_content() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        let missing = "0000000000000000000000000000000000000001";

        let error = read(repo.path(), &at("a.txt", missing)).unwrap_err();

        let file = repo.path().join("a.txt").display().to_string();
        assert_eq!(error.file, file);
        assert!(
            error
                .to_string()
                .starts_with(&format!("Could not load content for file {file}: ")),
            "{error}"
        );
        assert!(error.reason.contains(missing), "{error}");
    }

    #[test]
    fn a_path_missing_in_its_revision_or_on_disk_is_absent() {
        let repo = TempRepo::new();
        let head = repo.commit("a.txt", b"1\n", "one");

        assert_eq!(
            read(repo.path(), &at("gone.txt", &head)).unwrap(),
            ViewerContent::Absent
        );
        assert_eq!(
            read(repo.path(), &local("gone.txt")).unwrap(),
            ViewerContent::Absent
        );
    }

    #[test]
    fn a_path_outside_the_repository_is_not_read() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");

        assert!(read(repo.path(), &local("../a.txt")).is_err());
        assert!(read(repo.path(), &at("a.txt", "--output=x")).is_err());
    }

    #[test]
    fn edit_source_skips_local_files_that_are_not_on_disk() {
        let repo = TempRepo::new();
        let head = repo.commit("a.txt", b"1\n", "one");
        let targets = vec![local("a.txt"), local("gone.txt"), at("gone.txt", &head)];

        let kept = openable(repo.path(), targets).unwrap();

        assert_eq!(kept, vec![local("a.txt"), at("gone.txt", &head)]);
        assert!(openable(repo.path(), vec![local("../a.txt")]).is_err());
        assert!(openable(repo.path(), vec![at("a.txt", "HEAD")]).is_err());
    }

    fn canonical(path: &Path) -> String {
        fs::canonicalize(path).unwrap().display().to_string()
    }

    #[test]
    fn a_file_argument_opens_its_repository_and_the_viewer_at_the_line() {
        let repo = TempRepo::new();
        repo.commit("src/main file.rs", b"fn main() {}\n", "one");
        let file = repo.path().join("src/main file.rs").display().to_string();

        let plain = initial_target_of(Some(file.clone())).unwrap().unwrap();
        let at_line = initial_target_of(Some(format!("{file}:12")))
            .unwrap()
            .unwrap();

        let root = canonical(repo.path());
        assert_eq!(
            plain,
            InitialTarget {
                repo: root.clone(),
                file: Some("src/main file.rs".to_string()),
                line: None
            }
        );
        assert_eq!(
            at_line,
            InitialTarget {
                repo: root,
                file: Some("src/main file.rs".to_string()),
                line: Some(12)
            }
        );
    }

    #[test]
    fn a_folder_argument_still_opens_the_repository() {
        let repo = TempRepo::new();
        repo.commit("src/a.txt", b"1\n", "one");
        let folder = repo.path().join("src").display().to_string();

        let target = initial_target_of(Some(folder.clone())).unwrap();

        assert_eq!(
            target,
            Some(InitialTarget {
                repo: folder,
                file: None,
                line: None
            })
        );
        assert_eq!(initial_target_of(None).unwrap(), None);
        // What does not exist goes to `repo_open` as it is, which says why it cannot open it.
        let missing = repo.path().join("nothing:3").display().to_string();
        assert_eq!(
            initial_target_of(Some(missing.clone()))
                .unwrap()
                .unwrap()
                .repo,
            missing
        );
    }

    #[test]
    fn a_file_outside_a_repository_says_so() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("notes.txt");
        fs::write(&file, "1\n").unwrap();
        let file = file.display().to_string();

        let error = initial_target_of(Some(format!("{file}:2"))).unwrap_err();

        assert_eq!(error.kind, "not_a_repository");
        assert_eq!(
            error.message,
            format!("{file} is not inside a Git repository")
        );
    }
}
