// The writer: `PatchWriter` (order of the parts), `UnifiedDiffWriter`, `GitPatchWriter`,
// `BinaryPatchWriter` (headers only) and `StringUtil.escapeStringCharacters`.
use std::sync::LazyLock;

use super::*;

// `PatchWriter.write`: the `Subject:` header, the text files with hunks, the text files without
// (git header only), then the binary files. Always `\n` (decision 6).
pub fn write(patch: &Patch, options: &WriteOptions) -> Vec<u8> {
    let mut out = Vec::new();
    // `writeCommitMessageHeader`: nothing for an empty message (before trimming).
    let message = patch
        .header
        .message
        .as_deref()
        .or(patch.header.subject.as_deref());
    if let Some(message) = message.filter(|message| !message.is_empty()) {
        out.extend_from_slice(SUBJECT_HEADER.as_bytes());
        out.extend(to_lf(java_trim(message).as_bytes()));
        out.push(b'\n');
        out.extend_from_slice(HEADER_END_MARKER.as_bytes());
        out.push(b'\n');
    }
    let texts = || {
        patch.files.iter().filter_map(|file| match file {
            FilePatch::Text(text) => Some(text),
            FilePatch::Binary(_) => None,
        })
    };
    for file in texts().filter(|file| !file.hunks.is_empty()) {
        write_text(&mut out, file, options.include_base);
    }
    for file in texts().filter(|file| file.hunks.is_empty()) {
        write_git_header(
            &mut out,
            file.before.as_deref(),
            file.after.as_deref(),
            file.new_mode,
        );
    }
    for file in &patch.files {
        if let FilePatch::Binary(binary) = file {
            write_binary(&mut out, binary);
        }
    }
    out
}

fn push_line(out: &mut Vec<u8>, line: &str) {
    out.extend_from_slice(line.as_bytes());
    out.push(b'\n');
}

// `UnifiedDiffWriter.writeFileHeading` and the hunks.
fn write_text(out: &mut Vec<u8>, file: &TextFilePatch, include_base: bool) {
    let before = file.before.as_deref();
    let after = file.after.as_deref();
    let index_name = before.or(after).unwrap_or_default();
    push_line(out, &format!("Index: {}", quoted(index_name)));

    let mut additional = Vec::new();
    if include_base {
        let base = file
            .base
            .as_deref()
            .and_then(|base| std::str::from_utf8(base).ok());
        if let Some(base) = base.filter(|base| !base.is_empty()) {
            additional.push((BASE_SUBSYSTEM, base));
        }
    }
    if let Some(charset) = file
        .charset
        .as_deref()
        .filter(|charset| !charset.is_empty())
    {
        additional.push((CHARSET_SUBSYSTEM, charset));
    }
    if !additional.is_empty() {
        out.extend_from_slice(ADDITIONAL_PREFIX);
        out.push(b'\n');
        for (subsystem, value) in additional {
            out.extend_from_slice(ADD_INFO_HEADER);
            push_line(out, subsystem);
            // Escaped first, so there is no newline left to split on: one `<+>` line.
            for line in escape_string_characters(value)
                .split('\n')
                .filter(|line| !line.is_empty())
            {
                out.extend_from_slice(ADD_INFO_LINE_START);
                push_line(out, line);
            }
        }
    }
    push_line(out, HEADER_SEPARATOR);
    write_git_header(out, before, after, file.new_mode);
    let before_path = before.map_or(DEV_NULL.to_string(), |name| quoted(&format!("a/{name}")));
    let after_path = after.map_or(DEV_NULL.to_string(), |name| quoted(&format!("b/{name}")));
    write_revision_heading(out, "---", &before_path, file.before_rev.as_deref());
    write_revision_heading(out, "+++", &after_path, file.after_rev.as_deref());
    for hunk in &file.hunks {
        push_line(
            out,
            &format!(
                "@@ -{},{} +{},{} @@",
                hunk.before_start, hunk.before_count, hunk.after_start, hunk.after_count
            ),
        );
        for line in &hunk.lines {
            out.push(match line.kind {
                LineKind::Added => b'+',
                LineKind::Removed => b'-',
                LineKind::Context => b' ',
            });
            out.extend_from_slice(&line.text);
            if line.no_newline {
                out.push(b'\n');
                out.extend_from_slice(NO_NEWLINE_SIGNATURE);
            }
            out.push(b'\n');
        }
    }
}

// The TAB is always there, the version id only when it is not blank.
fn write_revision_heading(out: &mut Vec<u8>, prefix: &str, path: &str, version: Option<&str>) {
    let version = version
        .filter(|version| !java_trim(version).is_empty())
        .unwrap_or_default();
    push_line(out, &format!("{prefix} {path}\t{version}"));
}

// `GitPatchWriter.writeGitHeader`. A deleted file says `100644` whatever it was.
fn write_git_header(
    out: &mut Vec<u8>,
    before: Option<&str>,
    after: Option<&str>,
    new_mode: Option<u32>,
) {
    let before_name = before.or(after).unwrap_or_default();
    let after_name = after.or(before).unwrap_or_default();
    push_line(
        out,
        &format!(
            "diff --git {} {}",
            quoted(&format!("a/{before_name}")),
            quoted(&format!("b/{after_name}"))
        ),
    );
    if after.is_none() {
        push_line(out, &format!("deleted file mode {REGULAR_FILE_MODE:o}"));
    } else if before.is_none() {
        push_line(
            out,
            &format!("new file mode {:o}", new_mode.unwrap_or(REGULAR_FILE_MODE)),
        );
    } else if before_name != after_name {
        push_line(out, &format!("rename from {}", quoted(before_name)));
        push_line(out, &format!("rename to {}", quoted(after_name)));
    }
}

// `BinaryPatchWriter.writeBinaries`, with git's section (decision 3). Zeros stand for the hash of
// a side that does not exist.
fn write_binary(out: &mut Vec<u8>, file: &BinaryFilePatch) {
    write_git_header(
        out,
        file.before.as_deref(),
        file.after.as_deref(),
        file.new_mode,
    );
    if file.git_section.is_empty() {
        return;
    }
    let zeros = zero_hash(file.before_rev.as_deref().or(file.after_rev.as_deref()));
    let before_rev = file.before_rev.as_deref().unwrap_or(&zeros);
    let after_rev = file.after_rev.as_deref().unwrap_or(&zeros);
    push_line(out, &format!("index {before_rev}..{after_rev}"));
    out.extend_from_slice(&file.git_section);
    if !file.git_section.ends_with(b"\n") {
        out.push(b'\n');
    }
}

// IntelliJ writes names raw. Those that its reader (and git's) would take apart, with a `\`, a
// `"` or a control character, are written quoted as git does (C escapes, octal for the rest);
// non-ASCII characters and spaces stay raw.
fn quoted(name: &str) -> String {
    let needs_quotes = name
        .bytes()
        .any(|byte| byte < 0x20 || byte == 0x7f || byte == b'"' || byte == b'\\');
    if !needs_quotes {
        return name.to_string();
    }
    let mut result = String::from("\"");
    for character in name.chars() {
        match character {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\t' => result.push_str("\\t"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\u{7}' => result.push_str("\\a"),
            '\u{8}' => result.push_str("\\b"),
            '\u{c}' => result.push_str("\\f"),
            control if (control as u32) < 0x20 || control == '\u{7f}' => {
                result.push_str(&format!("\\{:03o}", control as u32));
            }
            other => result.push(other),
        }
    }
    result.push('"');
    result
}

// `StringUtil.escapeStringCharacters(s)`: on UTF-16 units, so each half of a surrogate pair is
// escaped on its own.
fn escape_string_characters(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for unit in text.encode_utf16() {
        match unit {
            0x08 => result.push_str("\\b"),
            0x09 => result.push_str("\\t"),
            0x0a => result.push_str("\\n"),
            0x0c => result.push_str("\\f"),
            0x0d => result.push_str("\\r"),
            0x5c => result.push_str("\\\\"),
            0x22 => result.push_str("\\\""),
            _ if !is_printable_unicode(unit) => result.push_str(&format!("\\u{unit:04X}")),
            _ => result.push(char::from_u32(u32::from(unit)).expect("not a surrogate")),
        }
    }
    result
}

// `StringUtil.isPrintableUnicode`: not unassigned, a line or paragraph separator, a control,
// format, private use or surrogate character, nor a variation selector. The categories are those
// of the `regex` crate's Unicode version, not those of IntelliJ's Java; for a character assigned
// in between, only the escape differs, and both forms read back the same.
fn is_printable_unicode(unit: u16) -> bool {
    static NOT_PRINTABLE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"^[\p{Cn}\p{Zl}\p{Zp}\p{Cc}\p{Cf}\p{Co}]$").expect("fixed pattern")
    });
    if (0xd800..=0xdfff).contains(&unit) || (0xfe00..=0xfe0f).contains(&unit) {
        return false;
    }
    let Some(character) = char::from_u32(u32::from(unit)) else {
        return false;
    };
    if character.is_ascii() {
        return !character.is_ascii_control();
    }
    let mut buffer = [0; 4];
    !NOT_PRINTABLE.is_match(character.encode_utf8(&mut buffer))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::patch::test_helpers::*;

    // Decision 11: one `<+>` line per subsystem, Base before Charset, and IntelliJ's escapes
    // (`\t`, `\\`, `\n`, `\"`, `\uXXXX` for each UTF-16 unit that is not printable).
    #[test]
    fn base_and_charset_lines_round_trip() {
        let repo = TempDir::new().unwrap();
        let base = "tab\there \\ back\nslash 😀 \"quote\" é \u{FEFF}bom\u{2028}end\n";
        let after = format!("{base}more\n");
        fs::write(repo.path().join("f.txt"), &after).unwrap();
        let mut options = options(repo.path());
        options.include_base = true;
        let entries = [modified("f.txt", base.as_bytes(), after.as_bytes())];
        let patch = build(repo.path(), &entries, &options).unwrap();
        let file = text(&patch.files[0]);
        assert_eq!(file.base.as_deref(), Some(base.as_bytes()));
        assert_eq!(file.charset.as_deref(), Some("UTF-8"));

        let written = String::from_utf8(write(&patch, &options)).unwrap();
        let expected_heading = concat!(
            "Index: f.txt\n",
            "IDEA additional info:\n",
            "Subsystem: com.intellij.openapi.diff.impl.patch.BaseRevisionTextPatchEP\n",
            "<+>tab\\there \\\\ back\\nslash \\uD83D\\uDE00 \\\"quote\\\" é \\uFEFFbom\\u2028end\\n",
            "\n",
            "Subsystem: com.intellij.openapi.diff.impl.patch.CharsetEP\n",
            "<+>UTF-8\n",
            "===================================================================\n",
            "diff --git a/f.txt b/f.txt\n",
        );
        assert!(written.starts_with(expected_heading), "{written}");

        let read_back = read(written.as_bytes()).unwrap();
        let file = text(&read_back.files[0]);
        assert_eq!(file.base.as_deref(), Some(base.as_bytes()));
        assert_eq!(file.charset.as_deref(), Some("UTF-8"));
        assert_eq!(read_back.additional_info_error, None);

        // Without `include_base`, only the charset.
        options.include_base = false;
        let written = String::from_utf8(write(&patch, &options)).unwrap();
        assert!(written.starts_with(concat!(
            "Index: f.txt\n",
            "IDEA additional info:\n",
            "Subsystem: com.intellij.openapi.diff.impl.patch.CharsetEP\n",
            "<+>UTF-8\n",
            "=====",
        )));

        // `unescapeStringCharacters`: octal, `\u`, unknown escapes, a lone `\` at the end, and
        // several `<+>` lines joined with `\n`. The base is read with `\n` separators, so the
        // `\r` at the end of the first line and the joining `\n` make one separator.
        let several = concat!(
            "IDEA additional info:\n",
            "Subsystem: com.intellij.openapi.diff.impl.patch.BaseRevisionTextPatchEP\n",
            "<+>\\101\\u0042\\x\\uZZ\\r",
            "\n",
            r"<+>last\r\n\",
            "\n",
            "===================================================================\n",
            "--- a/f.txt\n+++ b/f.txt\n@@ -1 +1 @@\n-a\n+b\n",
        );
        let read_back = read(several.as_bytes()).unwrap();
        assert_eq!(
            text(&read_back.files[0]).base.as_deref(),
            Some(&b"ABx\\uZZ\nlast\n\\"[..])
        );

        // Two blocks without a patch between them: V:1172, and no file keeps its info. (The line
        // right after a block is only tested as the start of a patch, so the second block must
        // come later to be seen, as in IntelliJ.)
        let twice = concat!(
            "IDEA additional info:\nSubsystem: com.intellij.openapi.diff.impl.patch.CharsetEP\n<+>UTF-8\n",
            "===================================================================\n",
            "IDEA additional info:\nSubsystem: com.intellij.openapi.diff.impl.patch.CharsetEP\n<+>UTF-8\n",
            "--- a/f.txt\n+++ b/f.txt\n@@ -1 +1 @@\n-a\n+b\n",
        );
        let read_back = read(twice.as_bytes()).unwrap();
        assert_eq!(
            read_back.additional_info_error.as_deref(),
            Some("Contains additional information without patch itself")
        );
        assert_eq!(text(&read_back.files[0]).charset, None);
    }
}
