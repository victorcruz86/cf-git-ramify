// Patches in IntelliJ's format. Read at the tag idea/241.14494.240.
//
// - Reader: `PatchReader` (parseAllPatches, AdditionalInfoParser, PatchContentParser,
//   isPatchContent), `PatchFileHeaderParser`, `GitPatchParser`, `BinaryPatchContentParser`,
//   `LineTokenizer` (tokenizeIntoList), `VcsFileUtil.unescapeGitPath` and
//   `StringUtil.unescapeStringCharacters`.
// - Writer: `PatchWriter` (order of the parts), `UnifiedDiffWriter`, `GitPatchWriter`,
//   `BinaryPatchWriter` (headers only) and `StringUtil.escapeStringCharacters`.
// - Builder: `TextPatchBuilder` and `IdeaTextPatchBuilder` (version ids), `CharsetEP` and
//   `BaseRevisionTextPatchEP` (the `IDEA additional info:` block).
//
// Decisions of the plan that change the port:
// - 3: the `GIT binary patch` sections are git's own (`git diff --binary` between two blobs) and
//   are kept raw on read; git applies them. Nothing here encodes or decodes base85 or zlib.
// - 4: the context-diff format is not read (`*** ` patches are refused).
// - 6: one encoding, UTF-8, and `\n` everywhere.
// - 11: the `<+>` lines follow rama's reading of IntelliJ's code.
//
// Lines of a patch and of the files are bytes: what is not UTF-8 goes through untouched. Names,
// version ids and the message are text.
mod build;
pub mod create;
mod read;
#[cfg(test)]
mod test_helpers;
mod write;

use std::path::PathBuf;

use regex::bytes::Regex;

use crate::git::GitError;

pub use build::build;
pub use read::{is_patch, read};
pub use write::write;

const REGULAR_FILE_MODE: u32 = 0o100644;
// `BlobIndexUtil.NOT_COMMITTED_HASH`: forty zeros.
const NOT_COMMITTED_HASH_LENGTH: usize = 40;

// `UnifiedDiffWriter`, `GitPatchWriter` and `BinaryPatchWriter`.
const SUBJECT_HEADER: &str = "Subject: [PATCH] ";
const HEADER_END_MARKER: &str = "---";
const ADDITIONAL_PREFIX: &[u8] = b"IDEA additional info:";
const ADD_INFO_HEADER: &[u8] = b"Subsystem: ";
const ADD_INFO_LINE_START: &[u8] = b"<+>";
const HEADER_SEPARATOR: &str =
    "===================================================================";
const NO_NEWLINE_SIGNATURE: &[u8] = b"\\ No newline at end of file";
const DEV_NULL: &str = "/dev/null";
const DIFF_GIT: &[u8] = b"diff --git";
const GIT_BINARY_HEADER: &[u8] = b"GIT binary patch";
// `PatchEP.getName` of the two extensions, in the order IntelliJ writes them.
const BASE_SUBSYSTEM: &str = "com.intellij.openapi.diff.impl.patch.BaseRevisionTextPatchEP";
const CHARSET_SUBSYSTEM: &str = "com.intellij.openapi.diff.impl.patch.CharsetEP";

// A whole patch file: the header (only for patches with a `Subject:`) and the files in it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Patch {
    pub header: Header,
    pub files: Vec<FilePatch>,
    // `PatchReader.getAdditionalInfo` fails with this message: IntelliJ then drops the
    // `IDEA additional info:` of every file, applies the rest and shows the error
    // (`ApplyPatchDefaultExecutor.applyAdditionalInfoBefore`). Here `base` and `charset` are
    // already empty.
    pub additional_info_error: Option<String>,
}

// `PatchFileHeaderInfo`. `message` is the whole message, from the `Subject:` line to the `---`
// line, trimmed; `subject` is the text of the `Subject:` line alone. `author` and
// `base_revision` come from `From:` and `From <hash>`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Header {
    pub subject: Option<String>,
    pub message: Option<String>,
    pub author: Option<Author>,
    pub base_revision: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Author {
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilePatch {
    Text(TextFilePatch),
    Binary(BinaryFilePatch),
}

// A text file. `before: None` is a new file and `after: None` a deleted one, as IntelliJ's
// `isNewFile` and `isDeletedFile` say (the status of the git header, or a single hunk that
// creates or removes the whole content). Otherwise both names are there, equal or not (a
// rename). The version ids are the text after the TAB of `---` and `+++`, as written
// (`(revision <hash>)`, `(date <ms>)`); in a git patch they are the hashes of its `index` line,
// or nothing. `new_mode` is the value of `new file mode` or `new mode`, for example `0o100755`.
// `base` and `charset` are the `IDEA additional info:` of the file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TextFilePatch {
    pub before: Option<String>,
    pub after: Option<String>,
    pub before_rev: Option<String>,
    pub after_rev: Option<String>,
    pub new_mode: Option<u32>,
    pub hunks: Vec<Hunk>,
    pub base: Option<Vec<u8>>,
    pub charset: Option<String>,
}

// The numbers of the `@@ -before_start,before_count +after_start,after_count @@` line, as
// written: the first line counts from 1, and an empty side starts at the line before which it
// would enter. `-0,0` is "no file" and `-1,0` "an empty file".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub before_start: usize,
    pub before_count: usize,
    pub after_start: usize,
    pub after_count: usize,
    pub lines: Vec<Line>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

// `text` has neither the prefix nor the line separator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub kind: LineKind,
    pub text: Vec<u8>,
    pub no_newline: bool,
}

// A binary file. The names and ids work as in `TextFilePatch`; the ids are the hashes of the
// `index` line. `git_section` holds the lines from `GIT binary patch` to the empty line after
// the last literal or delta, each one with its `\n`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BinaryFilePatch {
    pub before: Option<String>,
    pub after: Option<String>,
    pub before_rev: Option<String>,
    pub after_rev: Option<String>,
    pub new_mode: Option<u32>,
    pub git_section: Vec<u8>,
}

// `include_base` writes the `BaseRevisionTextPatchEP` lines (only the shelf does). `reverse` and
// `base_path` act on `build`: the sides are swapped before building, and the names are relative
// to `base_path` (relative to the repository when it is relative itself, the repository when
// empty).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WriteOptions {
    pub include_base: bool,
    pub reverse: bool,
    pub base_path: PathBuf,
}

// One change, with its paths relative to the repository and its contents. `before_rev` is the
// revision of the before content (HEAD's hash for local changes).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PatchSource {
    pub before: Option<(String, Vec<u8>)>,
    pub after: Option<(String, Vec<u8>)>,
    pub before_rev: Option<String>,
    pub binary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchError {
    // `PatchSyntaxException`: `line` is the index (from 0) of the line where reading stopped.
    Syntax { line: usize, message: String },
    Git(GitError),
    // An entry built as binary for which git printed a text diff (a repository attribute that wins
    // over rama's): the entry is refused rather than written as text, which would change its bytes.
    BinaryAsText { path: String },
}

impl From<GitError> for PatchError {
    fn from(error: GitError) -> Self {
        Self::Git(error)
    }
}

// A patch error as the commands report it.
pub(crate) fn patch_failure(error: PatchError) -> GitError {
    match error {
        PatchError::Git(error) => error,
        PatchError::Syntax { message, .. } => GitError::Io(message),
        // `build` refuses it rather than writing its bytes as text.
        PatchError::BinaryAsText { path } => GitError::Io(format!(
            "\"{path}\" is binary, but an attribute of the repository makes git show it as text: it cannot go into a patch"
        )),
    }
}

fn syntax(line: usize, message: impl Into<String>) -> PatchError {
    PatchError::Syntax {
        line,
        message: message.into(),
    }
}

//
// Lines and texts
//

// `LineTokenizer.tokenizeIntoList(text, false)`: `\n`, `\r\n` and a lone `\r` separate lines, the
// separators are dropped, and a separator at the end makes no empty last line.
fn split_lines(text: &[u8]) -> Vec<&[u8]> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < text.len() {
        match text[index] {
            b'\n' => {
                lines.push(&text[start..index]);
                index += 1;
                start = index;
            }
            b'\r' => {
                lines.push(&text[start..index]);
                index += 1;
                if text.get(index) == Some(&b'\n') {
                    index += 1;
                }
                start = index;
            }
            _ => index += 1,
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

// `StringUtil.convertLineSeparators`: what the document of a file holds.
fn to_lf(text: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        if text[index] == b'\r' {
            result.push(b'\n');
            if text.get(index + 1) == Some(&b'\n') {
                index += 1;
            }
        } else {
            result.push(text[index]);
        }
        index += 1;
    }
    result
}

// Java's `String.trim()`: every character up to the space.
fn java_trim(text: &str) -> &str {
    text.trim_matches(|c: char| c <= ' ')
}

fn java_trim_bytes(mut bytes: &[u8]) -> &[u8] {
    while let [first, rest @ ..] = bytes {
        if *first > b' ' {
            break;
        }
        bytes = rest;
    }
    while let [rest @ .., last] = bytes {
        if *last > b' ' {
            break;
        }
        bytes = rest;
    }
    bytes
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    find(haystack, needle).is_some()
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

// The hash of a side that does not exist: as many zeros as the other side's hash has digits
// (`BlobIndexUtil.NOT_COMMITTED_HASH`, forty, when neither has one).
fn zero_hash(other: Option<&str>) -> String {
    "0".repeat(other.map_or(NOT_COMMITTED_HASH_LENGTH, str::len))
}

fn pattern(text: &str) -> Regex {
    Regex::new(text).expect("fixed pattern")
}
