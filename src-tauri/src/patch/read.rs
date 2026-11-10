// The reader: `PatchReader`, `PatchFileHeaderParser`, `GitPatchParser`,
// `BinaryPatchContentParser`, `VcsFileUtil.unescapeGitPath` and
// `StringUtil.unescapeStringCharacters`.
use std::sync::LazyLock;

use regex::bytes::Regex;

use super::*;

// VcsBundle.properties, lines 422 and 1168 to 1174 (`patch.binary.decoder.char.error`,
// `patch.unknown.hunk.start.syntax`, `patch.second.file.name.expected`,
// `patch.empty.additional.info.header`, `patch.contains.additional.information.without.patch.itself`,
// `patch.can.t.detect.file.names.from.git.format.header.line`,
// `patch.unexpected.end.of.binary.patch`; `patch.empty.0.data.section` is written in place).
const BINARY_SIZE_CHAR: &str = "Cannot decode binary file patch: wrong char-size symbol";
const UNKNOWN_HUNK_START: &str = "Unknown chunk start syntax";
const SECOND_NAME_EXPECTED: &str = "Second file name expected";
const EMPTY_ADDITIONAL_INFO: &str = "Empty additional info header";
const INFO_WITHOUT_PATCH: &str = "Contains additional information without patch itself";
const NO_GIT_NAMES: &str = "Cannot detect file names from git format header line";
const BINARY_EOF: &str = "Unexpected end of binary patch";

// Ramify's own: what IntelliJ reads and Ramify does not, and what IntelliJ would only fail on later.
const CONTEXT_DIFF: &str = "Context diffs are not supported";
const UNKNOWN_BINARY_SECTION: &str = "Binary patch without literal or delta";
const NO_NAMES: &str = "File patch without file names";
const NOT_UTF8_NAME: &str = "File name that is not UTF-8";

// The `ListIterator` of `PatchReader`: `pos` is the index of the next line.
struct Lines<'a> {
    lines: Vec<&'a [u8]>,
    pos: usize,
}

impl<'a> Lines<'a> {
    fn has_next(&self) -> bool {
        self.pos < self.lines.len()
    }

    fn next(&mut self) -> &'a [u8] {
        let line = self.lines[self.pos];
        self.pos += 1;
        line
    }

    fn previous(&mut self) {
        self.pos -= 1;
    }

    fn previous_index(&self) -> usize {
        self.pos.saturating_sub(1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Unified,
    Context,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Start {
    Git,
    Unified,
    Context,
}

// `PatchContentParser.testIsStart`. The first non-git format seen fixes the format of the rest of
// the file; each git patch reads its content with a parser of its own (`format` starts empty).
fn start_of(line: &[u8], format: &mut Option<Format>) -> Option<Start> {
    if line.starts_with(DIFF_GIT) {
        return Some(Start::Git);
    }
    if line.starts_with(b"--- ") && *format != Some(Format::Context) {
        *format = Some(Format::Unified);
        return Some(Start::Unified);
    }
    if line.starts_with(b"*** ") && *format != Some(Format::Unified) {
        *format = Some(Format::Context);
        return Some(Start::Context);
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Status {
    #[default]
    Modified,
    Added,
    Deleted,
}

// A file as IntelliJ's `FilePatch` holds it, before `before`/`after` say new and deleted.
#[derive(Default)]
struct RawFile {
    before: Option<String>,
    after: Option<String>,
    before_rev: Option<String>,
    after_rev: Option<String>,
    status: Status,
    new_mode: Option<u32>,
    hunks: Vec<Hunk>,
    binary: Option<Vec<u8>>,
}

// `PatchReader.AdditionalInfoParser`: what is pending for the next patch, by subsystem.
#[derive(Default)]
struct AdditionalInfo {
    pending: Vec<(String, String)>,
    error: Option<String>,
}

impl AdditionalInfo {
    // `testIsStart`: `contains`, and nothing more after the first error.
    fn starts(&self, line: &[u8]) -> bool {
        self.error.is_none() && contains(line, ADDITIONAL_PREFIX)
    }

    fn fail(&mut self, message: impl Into<String>) {
        self.error = Some(message.into());
    }

    fn parse(&mut self, lines: &mut Lines) {
        if !lines.has_next() {
            self.fail(EMPTY_ADDITIONAL_INFO);
            return;
        }
        // IntelliJ calls `next()` here without asking (an exception at the end of the text);
        // here the block just ends.
        while lines.has_next() {
            let header = lines.next();
            let Some(index) = find(header, ADD_INFO_HEADER) else {
                if self.pending.is_empty() {
                    self.fail(EMPTY_ADDITIONAL_INFO);
                }
                lines.previous();
                return;
            };
            let subsystem = java_trim(&lossy(&header[index + ADD_INFO_HEADER.len()..])).to_string();
            if !lines.has_next() {
                self.fail(format!("Empty '{subsystem}' data section"));
                return;
            }
            let mut value = String::new();
            while lines.has_next() {
                let line = lines.next();
                if !line.starts_with(ADD_INFO_LINE_START) {
                    lines.previous();
                    break;
                }
                if !value.is_empty() {
                    value.push('\n');
                }
                value.push_str(&unescape_string_characters(&lossy(
                    &line[ADD_INFO_LINE_START.len()..],
                )));
            }
            self.pending.retain(|(name, _)| *name != subsystem);
            self.pending.push((subsystem, value));
        }
    }

    // `copyToResult`: the pending info goes to the patch just read (IntelliJ keys it by its name,
    // which is how its consumers find it). Only text files use it.
    fn give_to(&mut self, file: Option<&mut FilePatch>) {
        let pending = std::mem::take(&mut self.pending);
        let Some(FilePatch::Text(file)) = file else {
            return;
        };
        for (subsystem, value) in pending {
            if subsystem == BASE_SUBSYSTEM {
                // `BaseRevisionTextPatchEP.getBaseContent`: with `\n` separators.
                file.base = Some(to_lf(value.as_bytes()));
            } else if subsystem == CHARSET_SUBSYSTEM {
                file.charset = Some(value);
            }
        }
    }
}

// `PatchReader.parseAllPatches`. Context diffs are refused (decision 4).
pub fn read(text: &[u8]) -> Result<Patch, PatchError> {
    let mut lines = Lines {
        lines: split_lines(text),
        pos: 0,
    };
    let mut patch = Patch::default();
    let mut info = AdditionalInfo::default();
    let mut format = None;
    let mut contains_additional = false;
    let mut is_header_line = true;
    let mut header_lines = 0;
    while lines.has_next() {
        let mut next = lines.next();
        let contains_additional_now = info.starts(next);
        if contains_additional_now && contains_additional {
            info.fail(INFO_WITHOUT_PATCH);
        }
        if contains_additional_now {
            is_header_line = false;
            contains_additional = true;
            info.parse(&mut lines);
            if !lines.has_next() {
                info.fail(INFO_WITHOUT_PATCH);
                break;
            }
            next = lines.next();
        }
        if let Some(start) = start_of(next, &mut format) {
            is_header_line = false;
            let raw = match start {
                Start::Git => read_git_patch(next, &mut lines)?,
                Start::Unified => read_text_patch(next, &mut lines)?,
                Start::Context => return Err(syntax(lines.previous_index(), CONTEXT_DIFF)),
            };
            patch.files.push(finish(raw, lines.previous_index())?);
            if contains_additional {
                info.give_to(patch.files.last_mut());
            }
            contains_additional = false;
        }
        if is_header_line {
            header_lines += 1;
        }
    }
    if let Some(error) = info.error {
        for file in &mut patch.files {
            if let FilePatch::Text(file) = file {
                file.base = None;
                file.charset = None;
            }
        }
        patch.additional_info_error = Some(error);
    }
    patch.header = read_header(&lines.lines[..header_lines]);
    Ok(patch)
}

// `PatchFileHeaderParser.parseHeader`.
fn read_header(lines: &[&[u8]]) -> Header {
    static BASE_REVISION: LazyLock<Regex> =
        LazyLock::new(|| pattern(r"(?-u)^From\s+([0-9a-fA-F]+)\s+.*$"));
    static AUTHOR: LazyLock<Regex> =
        LazyLock::new(|| pattern(r"(?-u)^From:\s+(.*?)\s*(?:<(.*)>\s*)?$"));
    static SUBJECT: LazyLock<Regex> =
        LazyLock::new(|| pattern(r"(?-u)^Subject:(?:\s+\[PATCH.*\])?\s*(.+)$"));
    static HEADER_END: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?-u)^---\s*$"));

    let mut header = Header::default();
    let mut message = String::new();
    let mut whole_message = false;
    for line in lines {
        if HEADER_END.is_match(line) {
            break;
        } else if whole_message {
            message.push('\n');
            message.push_str(&lossy(line));
        } else if let Some(found) = BASE_REVISION.captures(line) {
            header.base_revision = Some(lossy(&found[1]));
        } else if let Some(found) = AUTHOR.captures(line) {
            header.author = Some(Author {
                name: lossy(&found[1]),
                email: found
                    .get(2)
                    .map(|email| lossy(email.as_bytes()))
                    .unwrap_or_default(),
            });
        } else if let Some(found) = SUBJECT.captures(line) {
            let subject = lossy(&found[1]);
            message.push_str(&subject);
            header.subject = Some(java_trim(&subject).to_string()).filter(|text| !text.is_empty());
            whole_message = true;
        }
    }
    let message = java_trim(&message);
    header.message = (!message.is_empty()).then(|| message.to_string());
    header
}

// `PatchContentParser.readTextPatch`, unified format only.
fn read_text_patch(first: &[u8], lines: &mut Lines) -> Result<RawFile, PatchError> {
    let mut file = RawFile::default();
    file_name(first, &mut file, true, lines.previous_index())?;
    if !lines.has_next() {
        return Err(syntax(lines.previous_index(), SECOND_NAME_EXPECTED));
    }
    let second = lines.next();
    if !second.starts_with(b"+++ ") {
        return Err(syntax(lines.previous_index(), SECOND_NAME_EXPECTED));
    }
    file_name(second, &mut file, false, lines.previous_index())?;
    while lines.has_next() {
        match next_hunk(lines)? {
            Some(hunk) => file.hunks.push(hunk),
            None => break,
        }
    }
    if file.before.is_none() {
        file.before = file.after.clone();
    }
    if file.after.is_none() {
        file.after = file.before.clone();
    }
    Ok(file)
}

// `@@ -s,n +s,n @@`, for the reader and for `is_patch`.
static UNIFIED_HUNK: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"(?-u)^@@ -([0-9]+)(,([0-9]+))? \+([0-9]+)(,([0-9]+))? @@.*$"));

// `readNextHunkUnified`. Lines that are no hunk are skipped up to the next `@@ `; a hunk ends
// when its counts are reached, so trailers like `-- ` after the last one are safe.
fn next_hunk(lines: &mut Lines) -> Result<Option<Hunk>, PatchError> {
    let mut current: &[u8] = b"";
    let mut read = 0;
    while lines.has_next() {
        current = lines.next();
        read += 1;
        if current.starts_with(b"--- ") || GIT_HEADER_SIMPLE.is_match(current) {
            lines.pos -= read;
            return Ok(None);
        }
        if current.starts_with(b"@@ ") {
            break;
        }
    }
    if !lines.has_next() {
        return Ok(None);
    }
    let wrong = || syntax(lines.previous_index(), UNKNOWN_HUNK_START);
    let found = UNIFIED_HUNK.captures(current).ok_or_else(wrong)?;
    let number = |group: usize, default: usize| match found.get(group) {
        None => Ok(default),
        Some(text) => lossy(text.as_bytes()).parse::<usize>().map_err(|_| wrong()),
    };
    let mut hunk = Hunk {
        before_start: number(1, 0)?,
        before_count: number(3, 1)?,
        after_start: number(4, 0)?,
        after_count: number(6, 1)?,
        lines: Vec::new(),
    };
    // Ramify's own limit: every line number of the hunk fits in a `u32`, so no consumer can overflow
    // on `start + count`.
    let fits = |start: usize, count: usize| {
        start
            .checked_add(count)
            .is_some_and(|end| end <= u32::MAX as usize)
    };
    if !fits(hunk.before_start, hunk.before_count) || !fits(hunk.after_start, hunk.after_count) {
        return Err(wrong());
    }
    let (mut before, mut after) = (0, 0);
    while lines.has_next() {
        let text = lines.next();
        if !hunk.lines.is_empty() && text.starts_with(NO_NEWLINE_SIGNATURE) {
            if let Some(last) = hunk.lines.last_mut() {
                last.no_newline = true;
            }
            continue;
        }
        let meaningful = before < hunk.before_count || after < hunk.after_count;
        let Some(line) = patch_line(text, meaningful) else {
            lines.previous();
            break;
        };
        match line.kind {
            LineKind::Context => {
                before += 1;
                after += 1;
            }
            LineKind::Added => after += 1,
            LineKind::Removed => before += 1,
        }
        hunk.lines.push(line);
    }
    Ok(Some(hunk))
}

// `parsePatchLine`: an empty line is an empty context line.
fn patch_line(text: &[u8], meaningful: bool) -> Option<Line> {
    if !meaningful {
        return None;
    }
    let kind = match text.first() {
        Some(b'+') => LineKind::Added,
        Some(b'-') => LineKind::Removed,
        None | Some(b' ') => LineKind::Context,
        Some(_) => return None,
    };
    Some(Line {
        kind,
        text: text.get(1..).unwrap_or_default().to_vec(),
        no_newline: false,
    })
}

// `extractFileName`: the name ends at the first TAB, or else at the first space; the rest is the
// version id unless it is `(revision)` with nothing in it.
fn file_name(
    line: &[u8],
    file: &mut RawFile,
    before: bool,
    index: usize,
) -> Result<(), PatchError> {
    static EMPTY_REVISION: LazyLock<Regex> =
        LazyLock::new(|| pattern(r"(?-u)^\(\s*revision\s*\)$"));
    let rest = &line[4..];
    let mut name = rest;
    let end = rest
        .iter()
        .position(|byte| *byte == b'\t')
        .or_else(|| rest.iter().position(|byte| *byte == b' '));
    if let Some(end) = end {
        let version = java_trim_bytes(&rest[end..]);
        name = &rest[..end];
        if !version.is_empty() && !EMPTY_REVISION.is_match(version) {
            let version = Some(lossy(version));
            if before {
                file.before_rev = version;
            } else {
                file.after_rev = version;
            }
        }
    }
    if let Some(name) = patch_name(name, before, index)? {
        if before {
            file.before = Some(name);
        } else {
            file.after = Some(name);
        }
    }
    Ok(())
}

// `unescapeGitPath` and then `stripPatchNameIfNeeded`: `/dev/null` is no name, and `a/` or `b/`
// goes away.
fn patch_name(text: &[u8], before: bool, index: usize) -> Result<Option<String>, PatchError> {
    let name = git_path(text, index)?;
    if name == DEV_NULL {
        return Ok(None);
    }
    let prefix = if before { "a/" } else { "b/" };
    Ok(Some(name.strip_prefix(prefix).unwrap_or(&name).to_string()))
}

// `VcsFileUtil.unescapeGitPath`, on bytes: the octal runs are the bytes of the UTF-8 name.
fn git_path(text: &[u8], index: usize) -> Result<String, PatchError> {
    let mut path = text;
    if path.len() >= 2 && path.starts_with(b"\"") && path.ends_with(b"\"") {
        path = &path[1..path.len() - 1];
    }
    let not_utf8 = || syntax(index, format!("{NOT_UTF8_NAME}: {}", lossy(text)));
    if !path.contains(&b'\\') {
        return String::from_utf8(path.to_vec()).map_err(|_| not_utf8());
    }
    let shown = lossy(path);
    let is_octal = |byte: u8| (b'0'..=b'7').contains(&byte);
    let mut result = Vec::with_capacity(path.len());
    let mut position = 0;
    while position < path.len() {
        let byte = path[position];
        if byte != b'\\' {
            result.push(byte);
            position += 1;
            continue;
        }
        position += 1;
        let Some(&escape) = path.get(position) else {
            return Err(syntax(
                index,
                format!("Unterminated escape sequence in the path: {shown}"),
            ));
        };
        let simple = match escape {
            b'\\' => Some(b'\\'),
            b't' => Some(b'\t'),
            b'n' => Some(b'\n'),
            b'r' => Some(b'\r'),
            b'a' => Some(0x07),
            b'b' => Some(0x08),
            b'f' => Some(0x0c),
            b'"' => Some(b'"'),
            _ => None,
        };
        if let Some(simple) = simple {
            result.push(simple);
            position += 1;
            continue;
        }
        if !is_octal(escape) {
            return Err(syntax(
                index,
                format!(
                    "Unknown escape sequence '\\{}' in the path: {shown}",
                    escape as char
                ),
            ));
        }
        // A run of `\ooo`, each up to three digits.
        loop {
            let mut code: u32 = 0;
            let mut digits = 0;
            while digits < 3 && position < path.len() && is_octal(path[position]) {
                code = code * 8 + u32::from(path[position] - b'0');
                position += 1;
                digits += 1;
            }
            result.push(code as u8);
            let next_is_octal = path.get(position) == Some(&b'\\')
                && path.get(position + 1).copied().is_some_and(is_octal);
            if !next_is_octal {
                break;
            }
            position += 1;
        }
    }
    String::from_utf8(result).map_err(|_| not_utf8())
}

static GIT_HEADER_SIMPLE: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"(?-u)^diff --git\s+(\S+)\s+(\S+).*$"));

// `GitPatchParser.parse`.
fn read_git_patch(start: &[u8], lines: &mut Lines) -> Result<RawFile, PatchError> {
    static AB_PREFIX: LazyLock<Regex> =
        LazyLock::new(|| pattern(r"(?-u)^diff --git a/(.+) b/(.+)$"));
    static QUOTED_AB_PREFIX: LazyLock<Regex> =
        LazyLock::new(|| pattern(r#"(?-u)^diff --git "a/(.+)" "b/(.+)"\s*$"#));
    // `..` is any two characters, as in IntelliJ's pattern.
    static INDEX: LazyLock<Regex> =
        LazyLock::new(|| pattern(r"(?-u)^index\s+([0-9a-fA-F]+)..([0-9a-fA-F]+).*$"));
    static RENAME_FROM: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?-u)^\s*rename from\s(.*)$"));
    static RENAME_TO: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?-u)^\s*rename to\s(.*)$"));
    static FILE_STATUS: LazyLock<Regex> =
        LazyLock::new(|| pattern(r"(?-u)^\s*(new|deleted)\s+file\s+mode\s*([0-9]*)\s*$"));
    static NEW_MODE: LazyLock<Regex> =
        LazyLock::new(|| pattern(r"(?-u)^\s*new\s+mode\s*([0-9]+)\s*$"));

    let index = lines.previous_index();
    // IntelliJ strips `a/` from the before name and `b/` from the after name again after the first
    // two patterns (which already matched them), so `diff --git a/a/x b/a/x` names the before side
    // `x` and leaves the after side `a/x`. Ramify strips them once, as git does; it matters for the
    // files whose names come only from this line (empty, mode only, binary).
    let (found, prefixed) = [
        (&*AB_PREFIX, false),
        (&*QUOTED_AB_PREFIX, false),
        (&*GIT_HEADER_SIMPLE, true),
    ]
    .iter()
    .find_map(|(names, prefixed)| Some((names.captures(start)?, *prefixed)))
    .ok_or_else(|| syntax(index, NO_GIT_NAMES))?;
    let header_name = |text: &[u8], before: bool| {
        if prefixed {
            return patch_name(text, before, index);
        }
        let name = git_path(text, index)?;
        Ok((name != DEV_NULL).then_some(name))
    };
    let mut before = header_name(&found[1], true)?;
    let mut after = header_name(&found[2], false)?;

    let mut status = Status::Modified;
    let mut new_mode = None;
    let mut hashes = None;
    let mut prefer_header_names = false;
    let mode = |digits: &[u8]| u32::from_str_radix(&lossy(digits), 8).ok();
    while lines.has_next() {
        let line = lines.next();
        if let Some(found) = FILE_STATUS.captures(line) {
            status = if &found[1] == b"new" {
                Status::Added
            } else {
                Status::Deleted
            };
            if !found[2].is_empty() && status == Status::Added {
                new_mode = mode(&found[2]);
            }
        } else if let Some(found) = NEW_MODE.captures(line) {
            new_mode = mode(&found[1]);
        } else if let Some(found) = INDEX.captures(line) {
            hashes = Some((lossy(&found[1]), lossy(&found[2])));
        } else if RENAME_FROM.is_match(line) && lines.has_next() {
            let from = RENAME_FROM.captures(line).expect("matched");
            let to_line = lines.next();
            match RENAME_TO.captures(to_line) {
                Some(to) => {
                    let index = lines.previous_index();
                    before = Some(git_path(java_trim_bytes(&from[1]), index)?);
                    after = Some(git_path(java_trim_bytes(&to[1]), index)?);
                    prefer_header_names = true;
                }
                None => lines.previous(),
            }
        } else if start_of(line, &mut None).is_some() || line.starts_with(GIT_BINARY_HEADER) {
            lines.previous();
            break;
        }
    }

    let mut file = RawFile::default();
    if lines.has_next() {
        let next = lines.next();
        if next.starts_with(GIT_BINARY_HEADER) {
            file.binary = Some(read_binary(lines)?);
        } else if next.starts_with(DIFF_GIT) {
            lines.previous();
        } else {
            match start_of(next, &mut None) {
                Some(Start::Unified) => file = read_text_patch(next, lines)?,
                Some(Start::Context) => {
                    return Err(syntax(lines.previous_index(), CONTEXT_DIFF));
                }
                Some(Start::Git) | None => {}
            }
        }
    }
    // `applyPatchInfo`.
    file.status = status;
    if file.before.is_none() || prefer_header_names {
        file.before = before;
    }
    if file.after.is_none() || prefer_header_names {
        file.after = after;
    }
    let (before_rev, after_rev) = hashes.unzip();
    file.before_rev = before_rev;
    file.after_rev = after_rev;
    file.new_mode = new_mode;
    Ok(file)
}

// `BinaryPatchContentParser.readGitBinaryFormatPatch`, keeping the lines instead of decoding them:
// the forward literal or delta, then, after an optional empty line, the reverse one if there is
// one (IntelliJ reads it only for files that are not new, git writes it for all).
fn read_binary(lines: &mut Lines) -> Result<Vec<u8>, PatchError> {
    if !lines.has_next() {
        return Err(syntax(lines.previous_index(), BINARY_EOF));
    }
    let mut section = GIT_BINARY_HEADER.to_vec();
    section.push(b'\n');
    binary_block(lines, &mut section)?;
    if lines.has_next() {
        let mut next = lines.next();
        if java_trim_bytes(next).is_empty() && lines.has_next() {
            next = lines.next();
        }
        lines.previous();
        if BINARY_BLOCK.is_match(next) {
            binary_block(lines, &mut section)?;
        }
    }
    Ok(section)
}

static BINARY_BLOCK: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"(?-u)^(literal|delta)\s+([0-9]+)\s*$"));

// One `literal N` or `delta N` and its data lines, up to an empty line or the end. Each data line
// starts with its length (`A`-`Z` for 1-26, `a`-`z` for 27-52, `BinaryEncoder`); git checks the
// rest when it applies the section.
fn binary_block(lines: &mut Lines, section: &mut Vec<u8>) -> Result<(), PatchError> {
    let header = lines.next();
    if !BINARY_BLOCK.is_match(header) {
        return Err(syntax(lines.previous_index(), UNKNOWN_BINARY_SECTION));
    }
    section.extend_from_slice(header);
    section.push(b'\n');
    if !lines.has_next() {
        return Err(syntax(lines.previous_index(), BINARY_EOF));
    }
    while lines.has_next() {
        let line = lines.next();
        if line.is_empty() {
            break;
        }
        if !line[0].is_ascii_alphabetic() {
            return Err(syntax(lines.previous_index(), BINARY_SIZE_CHAR));
        }
        section.extend_from_slice(line);
        section.push(b'\n');
    }
    section.push(b'\n');
    Ok(())
}

fn is_new_content(hunk: &Hunk) -> bool {
    hunk.before_start == 0 && hunk.before_count == 0
}

fn is_deleted_content(hunk: &Hunk) -> bool {
    hunk.after_start == 0 && hunk.after_count == 0
}

// From IntelliJ's `FilePatch` to Ramify's: `isNewFile` leaves no before name and `isDeletedFile`
// no after name (`TextFilePatch.java:57-70`; for a binary file, its status).
fn finish(raw: RawFile, index: usize) -> Result<FilePatch, PatchError> {
    let single = |test: fn(&Hunk) -> bool| raw.hunks.len() == 1 && test(&raw.hunks[0]);
    let is_new = raw.status == Status::Added || (raw.binary.is_none() && single(is_new_content));
    let is_deleted =
        raw.status == Status::Deleted || (raw.binary.is_none() && single(is_deleted_content));
    let mut before = raw.before.clone().or_else(|| raw.after.clone());
    let mut after = raw.after.clone().or_else(|| raw.before.clone());
    if after.is_none() {
        return Err(syntax(index, NO_NAMES));
    }
    if is_new {
        before = None;
    } else if is_deleted {
        after = None;
    }
    Ok(match raw.binary {
        Some(git_section) => FilePatch::Binary(BinaryFilePatch {
            before,
            after,
            before_rev: raw.before_rev,
            after_rev: raw.after_rev,
            new_mode: raw.new_mode,
            git_section,
        }),
        None => FilePatch::Text(TextFilePatch {
            before,
            after,
            before_rev: raw.before_rev,
            after_rev: raw.after_rev,
            new_mode: raw.new_mode,
            hunks: raw.hunks,
            base: None,
            charset: None,
        }),
    })
}

// `PatchReader.isPatchContent`: the format from the file lines, then at least one hunk. A git
// header alone (a rename, a binary file) is not a patch here.
pub fn is_patch(text: &[u8]) -> bool {
    static CONTEXT_BEFORE_HUNK: LazyLock<Regex> =
        LazyLock::new(|| pattern(r"(?-u)^\*\*\* ([0-9]+),([0-9]+) \*\*\*\*$"));
    static CONTEXT_AFTER_HUNK: LazyLock<Regex> =
        LazyLock::new(|| pattern(r"(?-u)^--- ([0-9]+),([0-9]+) ----$"));
    let lines = split_lines(text);
    let mut rest = lines.iter();
    let mut format = None;
    for line in rest.by_ref() {
        if line.starts_with(b"***************") {
            format = Some(Format::Context);
        } else if format == Some(Format::Context) && CONTEXT_BEFORE_HUNK.is_match(line) {
            break;
        } else if line.starts_with(b"--- ") && format.is_none() {
            format = Some(Format::Unified);
        } else if format == Some(Format::Unified) && line.starts_with(b"+++ ") {
            break;
        }
    }
    let hunk = match format {
        None => return false,
        Some(Format::Context) => &*CONTEXT_AFTER_HUNK,
        Some(Format::Unified) => &*UNIFIED_HUNK,
    };
    rest.any(|line| hunk.is_match(line))
}

// `StringUtil.unescapeStringCharacters`, on UTF-16 units so that two `\uXXXX` halves make one
// character again.
fn unescape_string_characters(text: &str) -> String {
    let characters: Vec<char> = text.chars().collect();
    let mut units: Vec<u16> = Vec::with_capacity(text.len());
    let mut buffer = [0; 2];
    let mut push = |units: &mut Vec<u16>, character: char| {
        units.extend_from_slice(character.encode_utf16(&mut buffer));
    };
    let mut escaped = false;
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        if !escaped {
            if character == '\\' {
                escaped = true;
            } else {
                push(&mut units, character);
            }
            index += 1;
            continue;
        }
        match character {
            'n' => units.push(0x0a),
            'r' => units.push(0x0d),
            'b' => units.push(0x08),
            't' => units.push(0x09),
            'f' => units.push(0x0c),
            '\'' | '"' | '\\' => push(&mut units, character),
            'u' => {
                // `Integer.parseInt(…, 16)` of the next four, kept as a 16-bit `char`.
                let code: Option<String> = characters
                    .get(index + 1..index + 5)
                    .map(|code| code.iter().collect());
                match code.and_then(|code| i32::from_str_radix(&code, 16).ok()) {
                    Some(code) => {
                        units.push(code as u16);
                        index += 4;
                    }
                    None => units.extend("\\u".encode_utf16()),
                }
            }
            '0'..='7' => {
                let longest = if character <= '3' { 3 } else { 2 };
                let mut end = index + 1;
                while end < characters.len()
                    && end < index + longest
                    && ('0'..='7').contains(&characters[end])
                {
                    end += 1;
                }
                let digits: String = characters[index..end].iter().collect();
                units.push(u16::from_str_radix(&digits, 8).expect("octal digits"));
                index = end - 1;
            }
            other => push(&mut units, other),
        }
        escaped = false;
        index += 1;
    }
    if escaped {
        units.push(u16::from(b'\\'));
    }
    String::from_utf16_lossy(&units)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;
    use crate::patch::test_helpers::*;
    use crate::test_support::Random;

    // Case, subject, message, author (name and email) and base revision.
    type HeaderCase<'a> = (
        &'a str,
        Option<&'a str>,
        Option<&'a str>,
        Option<(&'a str, &'a str)>,
        Option<&'a str>,
    );

    // `PatchReaderTest`, with its expected values.
    #[test]
    fn reader_parses_headers_and_subject() {
        let author = Some(("D D", "aaaa@gmail.com"));
        let base = Some("d48bebc211cc216aaa78bdf25d7f0b0143d6333b");
        let subject = Some("Subject line");
        let cases: [HeaderCase; 11] = [
            ("patchHeader", subject, subject, author, base),
            (
                "patchHeaderWithMultiLineMessage",
                subject,
                Some("Subject line\n\n* another line"),
                author,
                base,
            ),
            (
                "patchHeaderMessageWithNoPatchPart",
                subject,
                subject,
                author,
                base,
            ),
            ("patchHeaderWithoutBase", subject, subject, author, None),
            ("patchHeaderWithoutAuthor", subject, subject, None, base),
            ("patchHeaderWithoutMessage", None, None, author, base),
            ("patchHeaderWithoutAdditionalInfo", None, None, None, None),
            (
                "patchHeaderAdditionalInfoInWrongPlace",
                None,
                None,
                None,
                None,
            ),
            ("patchHeaderWithNoStateInfo", subject, subject, author, base),
            (
                "patchHeaderWithDoubleSurname",
                subject,
                subject,
                Some(("D D-D", "aaaa@gmail.com")),
                base,
            ),
            (
                "patchHeaderWithLongName",
                subject,
                subject,
                Some(("very long author-surname", "aaaa@gmail.com")),
                base,
            ),
        ];
        let root = fixtures("patchReader");
        assert_eq!(cases.len() + 2, self::cases(&root).len());
        for (case, subject, message, author, base) in cases {
            let patch = read(&fs::read(root.join(case).join("test.patch")).unwrap()).unwrap();
            let expected = Header {
                subject: subject.map(str::to_string),
                message: message.map(str::to_string),
                author: author.map(|(name, email)| Author {
                    name: name.to_string(),
                    email: email.to_string(),
                }),
                base_revision: base.map(str::to_string),
            };
            assert_eq!(patch.header, expected, "{case}");
        }
        for (case, count) in [
            ("patchWithFileModeChangedOnly", 1),
            ("patchWithFileModeAndOther", 2),
        ] {
            let patch = read(&fs::read(root.join(case).join("test.patch")).unwrap()).unwrap();
            assert_eq!(patch.files.len(), count, "{case}");
        }

        // `[PATCH.*]` is greedy, and every line up to `---` belongs to the message.
        let patch = read(b"Subject: [PATCH] fix [x] y\n\nbody\n---\n").unwrap();
        assert_eq!(patch.header.subject.as_deref(), Some("y"));
        assert_eq!(patch.header.message.as_deref(), Some("y\n\nbody"));

        // The writer's header: the message trimmed, with its separators as `\n`.
        let mut patch = Patch::default();
        patch.header.message = Some("  Fix it\r\n\r\nbody \n".to_string());
        assert_eq!(
            write(&patch, &options(Path::new("/"))),
            b"Subject: [PATCH] Fix it\n\nbody\n---\n"
        );
        let read_back = read(&write(&patch, &options(Path::new("/")))).unwrap();
        assert_eq!(read_back.header.message.as_deref(), Some("Fix it\n\nbody"));
        assert_eq!(read_back.header.subject.as_deref(), Some("Fix it"));

        // An old IntelliJ patch: no git header, the version ids kept, `(revision )` is none.
        let patch =
            read(&fs::read(fixtures("patchTextDetection/ideaPatch/test.patch")).unwrap()).unwrap();
        let file = text(&patch.files[0]);
        assert!(file
            .before
            .as_deref()
            .unwrap()
            .ends_with("text/FileDropHandler.java"));
        assert_eq!(file.before, file.after);
        assert_eq!(file.before_rev.as_deref(), Some("(date 1461158849000)"));
        assert_eq!(file.after_rev, None);
        assert_eq!(file.charset.as_deref(), Some("UTF-8"));

        // Syntax errors stop the whole read, with V:1168 and V:1169.
        let wrong_hunk = read(b"--- a/x\n+++ b/x\n@@ -a +1 @@\n-x\n").unwrap_err();
        assert_eq!(
            wrong_hunk,
            PatchError::Syntax {
                line: 2,
                message: "Unknown chunk start syntax".to_string()
            }
        );
        let no_second = read(b"--- a/x\nsomething\n").unwrap_err();
        assert!(
            matches!(no_second, PatchError::Syntax { ref message, .. } if message == "Second file name expected")
        );
        // Text around the patches is no error, and a hunk stops at its counts.
        let patch = read(b"intro\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n-- \n2.39.0\n").unwrap();
        assert_eq!(text(&patch.files[0]).hunks[0].lines.len(), 2);
    }

    // `PatchTextDetectionTest`.
    #[test]
    fn patch_text_detection_matches_intellij() {
        let root = fixtures("patchTextDetection");
        let cases = [
            ("classicalContextDiff", true),
            ("classicalUnifiedDiff", true),
            ("contextDiffWithExtraInfo", true),
            ("ideaPatch", true),
            ("normalDiff", false),
            ("randomText", false),
        ];
        assert_eq!(cases.len(), self::cases(&root).len());
        for (case, expected) in cases {
            let text = fs::read(root.join(case).join("test.patch")).unwrap();
            assert_eq!(is_patch(&text), expected, "{case}");
        }
        // A git header alone is not enough (rename only, binary only).
        assert!(!is_patch(
            b"diff --git a/x b/y\nsimilarity index 100%\nrename from x\nrename to y\n"
        ));
        assert!(!is_patch(b""));
    }

    // Ramify's own errors (the deviations where IntelliJ crashes, misreads or reads what Ramify does
    // not), with the line where reading stopped.
    #[test]
    fn ramify_errors_and_garbage_never_panic() {
        let cases: [(&[u8], usize, &str); 10] = [
            (
                b"diff --git a/x b/x\nGIT binary patch\nfoo\n",
                2,
                UNKNOWN_BINARY_SECTION,
            ),
            (b"diff --git a/x b/x\nGIT binary patch", 1, BINARY_EOF),
            (
                b"diff --git a/x b/x\nGIT binary patch\nliteral 1\n!abc\n",
                3,
                BINARY_SIZE_CHAR,
            ),
            (
                b"*** a\n--- b\n***************\n*** 1,1 ****\n",
                0,
                CONTEXT_DIFF,
            ),
            (
                b"--- /dev/null\n+++ /dev/null\n@@ -1 +1 @@\n-a\n+b\n",
                4,
                NO_NAMES,
            ),
            (
                b"--- a/\xff\t\n+++ b/\xff\t\n@@ -1 +1 @@\n-a\n+b\n",
                0,
                NOT_UTF8_NAME,
            ),
            (
                b"--- a/x\\\t\n+++ b/x\t\n",
                0,
                "Unterminated escape sequence in the path: a/x\\",
            ),
            (
                b"--- a/x\\q\t\n+++ b/x\t\n",
                0,
                "Unknown escape sequence '\\q' in the path: a/x\\q",
            ),
            (b"diff --git x\n", 0, NO_GIT_NAMES),
            (
                b"--- a/x\n+++ b/x\n@@ -1,1 +1,x @@\n-a\n",
                2,
                UNKNOWN_HUNK_START,
            ),
        ];
        for (input, line, message) in cases {
            match read(input) {
                Err(PatchError::Syntax {
                    line: found_line,
                    message: found,
                }) => {
                    assert!(
                        found.starts_with(message),
                        "{found:?} for {:?}",
                        lossy(input)
                    );
                    assert_eq!(found_line, line, "{:?}", lossy(input));
                }
                other => panic!("{other:?} for {:?}", lossy(input)),
            }
        }

        // Text that is no patch: nothing read, and not a patch for the clipboard.
        let prose = b"Dear all,\n-- \nregards\n+++ more\n@@ nothing @@\n";
        assert_eq!(read(prose).unwrap(), Patch::default());
        assert!(!is_patch(prose));
        // A block of additional info at the end of the text: an error kept, not a failed read.
        let at_end = read(b"IDEA additional info:\nSubsystem: x\n<+>y").unwrap();
        assert_eq!(
            at_end.additional_info_error.as_deref(),
            Some(INFO_WITHOUT_PATCH)
        );

        // Several MB of random bytes mixed with pieces of patch syntax: any result, no panic.
        const PIECES: [&[u8]; 12] = [
            b"diff --git a/x b/x\n",
            b"--- a/x\t(revision 1)\n",
            b"+++ b/x\n",
            b"@@ -1,2 +1,3 @@\n",
            b"@@ -0,0 +4294967295,9 @@\n",
            b"GIT binary patch\n",
            b"literal 5\n",
            b"delta 3\n",
            b"IDEA additional info:\n",
            b"Subsystem: com.intellij.openapi.diff.impl.patch.BaseRevisionTextPatchEP\n",
            b"<+>\\uD83D\\u12\\377\\\n",
            b"\\ No newline at end of file\n",
        ];
        let mut random = Random(4);
        let mut garbage = Vec::new();
        while garbage.len() < 6_000_000 {
            if random.below(3) == 0 {
                garbage.extend_from_slice(PIECES[random.below(PIECES.len() as u32) as usize]);
            } else {
                let length = random.below(200);
                garbage.extend((0..length).map(|_| random.below(256) as u8));
            }
        }
        // A read stops at its first syntax error, so the slices reach much more of it.
        let _ = read(&garbage);
        let _ = is_patch(&garbage);
        for slice in garbage.chunks(64 * 1024) {
            let _ = read(slice);
            let _ = is_patch(slice);
        }
    }

    // A count past `u32::MAX` is refused when read, so no consumer can overflow on it.
    #[test]
    fn hunk_numbers_past_u32_are_refused() {
        for header in [
            "@@ -4294967295,1 +1,1 @@",
            "@@ -1,4294967295 +1,1 @@",
            "@@ -1,1 +4294967296,0 @@",
            "@@ -18446744073709551615,5 +1,1 @@",
        ] {
            let text = format!("--- a/x\n+++ b/x\n{header}\n-a\n+b\n");
            assert_eq!(
                read(text.as_bytes()),
                Err(syntax(2, UNKNOWN_HUNK_START)),
                "{header}"
            );
        }
        let text = "--- a/x\n+++ b/x\n@@ -4294967294,1 +1,1 @@\n-a\n+b\n";
        assert_eq!(
            text_of(&read(text.as_bytes()).unwrap()).hunks[0].before_start,
            4294967294
        );
    }
}
