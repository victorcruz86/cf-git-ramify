// The builder: `TextPatchBuilder` and `IdeaTextPatchBuilder` (version ids), `CharsetEP` and
// `BaseRevisionTextPatchEP` (the `IDEA additional info:` block).
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path};
use std::time::{Duration, Instant, UNIX_EPOCH};

use similar::{capture_diff_slices_deadline, Algorithm, DiffTag};

use super::*;
use crate::git::{self, GitError};

// `TextPatchBuilder.CONTEXT_LINES`, the default of the `patch.context.line.count` registry key.
const CONTEXT_LINES: usize = 3;
// How long the line diff of one file may take before it settles for a longer diff.
const DIFF_DEADLINE: Duration = Duration::from_secs(2);
// `VcsConfiguration.ourMaximumFileForBaseRevisionSize`.
const MAX_BASE_SIZE: usize = 500_000;
const EXECUTABLE_FILE_MODE: u32 = 0o100755;

// One side of a change: its path in the repository, its name in the patch and its version id.
struct Side<'a> {
    path: &'a str,
    name: String,
    content: &'a [u8],
    version: Option<String>,
}

impl<'a> Side<'a> {
    fn new(
        repo: &Path,
        base: &Path,
        source: &'a (String, Vec<u8>),
        version: Option<String>,
    ) -> Self {
        let (path, content) = source;
        Self {
            path,
            name: relative_name(base, &repo.join(path)),
            content,
            version,
        }
    }
}

pub fn build(
    repo: &Path,
    entries: &[PatchSource],
    options: &WriteOptions,
) -> Result<Patch, PatchError> {
    build_with_context(repo, entries, options, CONTEXT_LINES)
}

// `TextPatchBuilder.build`. Version ids (`getRevisionName`): `(revision <before_rev>)` for the
// before side, `(date <ms>)` of the file on disk for the after side. A new file takes both from
// its after side and a deleted one from its before side.
fn build_with_context(
    repo: &Path,
    entries: &[PatchSource],
    options: &WriteOptions,
    context: usize,
) -> Result<Patch, PatchError> {
    let base = repo.join(&options.base_path);
    let mut blobs = Blobs::new(repo);
    let mut files = Vec::new();
    for entry in entries {
        let revision = entry
            .before_rev
            .as_deref()
            .filter(|revision| !revision.trim().is_empty())
            .map(|revision| format!("(revision {revision})"));
        let before = entry
            .before
            .as_ref()
            .map(|before| Side::new(repo, &base, before, revision));
        let after = entry.after.as_ref().map(|after| {
            let date = date_name(&repo.join(&after.0));
            Side::new(repo, &base, after, date)
        });
        let (before, after) = if options.reverse {
            (after, before)
        } else {
            (before, after)
        };
        let file = if entry.binary {
            build_binary(repo, &mut blobs, before, after)?
        } else {
            build_text(repo, before, after, options.include_base, context).map(FilePatch::Text)
        };
        files.extend(file);
    }
    Ok(Patch {
        files,
        ..Patch::default()
    })
}

fn date_name(file: &Path) -> Option<String> {
    let modified = fs::metadata(file).ok()?.modified().ok()?;
    let millis = modified.duration_since(UNIX_EPOCH).ok()?.as_millis();
    Some(format!("(date {millis})"))
}

// `basePath.relativize(file)` with `/`, without touching the disk.
fn relative_name(base: &Path, file: &Path) -> String {
    fn normal(path: &Path) -> Vec<Component<'_>> {
        let mut parts = Vec::new();
        for part in path.components() {
            match part {
                Component::CurDir => {}
                Component::ParentDir if matches!(parts.last(), Some(Component::Normal(_))) => {
                    parts.pop();
                }
                other => parts.push(other),
            }
        }
        parts
    }
    let base = normal(base);
    let file = normal(file);
    let common = base
        .iter()
        .zip(&file)
        .take_while(|(one, other)| one == other)
        .count();
    let mut parts: Vec<String> = vec!["..".to_string(); base.len() - common];
    parts.extend(
        file[common..]
            .iter()
            .map(|part| part.as_os_str().to_string_lossy().into_owned()),
    );
    parts.join("/")
}

// The executable bit of the new file on disk (`GitPatchWriter.writeGitHeader`).
fn mode_on_disk(file: &Path) -> u32 {
    match fs::metadata(file) {
        Ok(metadata) if metadata.permissions().mode() & 0o111 != 0 => EXECUTABLE_FILE_MODE,
        _ => REGULAR_FILE_MODE,
    }
}

// `CharsetEP.provideContent`: the charset of a file that is in the project (decision 6: UTF-8).
// Only files with hunks write it, so only they get it.
fn charset_of(repo: &Path, path: &str) -> Option<String> {
    repo.join(path).exists().then(|| "UTF-8".to_string())
}

// `buildAddedFile`, `buildDeletedFile` and `buildModifiedFile`. The contents are read with `\n`
// separators, as a document holds them, so a CRLF file gives a patch with `\n`.
fn build_text(
    repo: &Path,
    before: Option<Side>,
    after: Option<Side>,
    include_base: bool,
    context: usize,
) -> Option<TextFilePatch> {
    let with_charset = |mut file: TextFilePatch, path: &str| {
        if !file.hunks.is_empty() {
            file.charset = charset_of(repo, path);
        }
        file
    };
    match (before, after) {
        (None, None) => None,
        (None, Some(after)) => {
            let content = to_lf(after.content);
            let file = TextFilePatch {
                before: None,
                after: Some(after.name),
                before_rev: after.version.clone(),
                after_rev: after.version,
                new_mode: Some(mode_on_disk(&repo.join(after.path))),
                hunks: whole_file_hunk(&content, true, false).into_iter().collect(),
                ..TextFilePatch::default()
            };
            Some(with_charset(file, after.path))
        }
        (Some(before), None) => {
            let content = to_lf(before.content);
            let file = TextFilePatch {
                before: Some(before.name),
                after: None,
                before_rev: before.version.clone(),
                after_rev: before.version,
                hunks: whole_file_hunk(&content, false, false)
                    .into_iter()
                    .collect(),
                ..TextFilePatch::default()
            };
            Some(with_charset(file, before.path))
        }
        (Some(before), Some(after)) => {
            let hunks = build_hunks(&to_lf(before.content), &to_lf(after.content), context);
            // An unchanged file with the same name gives no patch.
            if hunks.is_empty() && before.name == after.name {
                return None;
            }
            // `ShelveChangesManager.baseRevisionsOfDvcsIntoContext`: modified files up to 500,000
            // bytes, found by the after name (so a renamed file has none). Decision 6: UTF-8 only.
            // With `\n` separators, as `BaseRevisionTextPatchEP.getBaseContent` reads it back.
            let base = (include_base
                && !hunks.is_empty()
                && before.name == after.name
                && after.content.len() <= MAX_BASE_SIZE
                && std::str::from_utf8(before.content).is_ok())
            .then(|| to_lf(before.content));
            let file = TextFilePatch {
                before: Some(before.name),
                after: Some(after.name),
                before_rev: before.version,
                after_rev: after.version,
                hunks,
                base,
                ..TextFilePatch::default()
            };
            Some(with_charset(file, after.path))
        }
    }
}

// `TextPatchBuilder.createWholeFileHunk`. `with_empty_file` is a modification from or to an empty
// file (`-1,0`); otherwise the other side does not exist (`-0,0`). An empty content has no hunk.
fn whole_file_hunk(content: &[u8], insertion: bool, with_empty_file: bool) -> Option<Hunk> {
    if content.is_empty() {
        return None;
    }
    let lines = split_lines(content);
    let no_newline = !content.ends_with(b"\n");
    let empty_start = usize::from(with_empty_file);
    let kind = if insertion {
        LineKind::Added
    } else {
        LineKind::Removed
    };
    let count = lines.len();
    Some(Hunk {
        before_start: if insertion { empty_start } else { 1 },
        before_count: if insertion { 0 } else { count },
        after_start: if insertion { 1 } else { empty_start },
        after_count: if insertion { count } else { 0 },
        lines: lines
            .iter()
            .enumerate()
            .map(|(index, text)| Line {
                kind,
                text: text.to_vec(),
                no_newline: no_newline && index + 1 == count,
            })
            .collect(),
    })
}

// A changed range of lines, from 0 and half-open (`com.intellij.diff.util.Range`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Range {
    start1: usize,
    end1: usize,
    start2: usize,
    end2: usize,
}

// `TextPatchBuilder.buildPatchHunks`.
fn build_hunks(before: &[u8], after: &[u8], context: usize) -> Vec<Hunk> {
    if before == after {
        return Vec::new();
    }
    if before.is_empty() {
        return whole_file_hunk(after, true, true).into_iter().collect();
    }
    if after.is_empty() {
        return whole_file_hunk(before, false, true).into_iter().collect();
    }
    let lines1 = split_lines(before);
    let lines2 = split_lines(after);
    let no_newline1 = !before.ends_with(b"\n");
    let no_newline2 = !after.ends_with(b"\n");
    let fragments = compare_lines(&lines1, &lines2, no_newline1, no_newline2);
    let mut hunks = Vec::new();
    let mut start = 0;
    while start < fragments.len() {
        // `getAdjacentFragments`: apart only with more than twice the context on both sides.
        let mut end = start + 1;
        while end < fragments.len() {
            let last = fragments[end - 1];
            let next = fragments[end];
            if last.end1 + context < next.start1.saturating_sub(context)
                && last.end2 + context < next.start2.saturating_sub(context)
            {
                break;
            }
            end += 1;
        }
        let sides = [(&lines1[..], no_newline1), (&lines2[..], no_newline2)];
        hunks.push(create_hunk(&fragments[start..end], sides, context));
        start = end;
    }
    hunks
}

// `TextPatchBuilder.createHunk`: for each range, the context before it, all its removed lines,
// then all its added ones.
fn create_hunk(fragments: &[Range], sides: [(&[&[u8]], bool); 2], context: usize) -> Hunk {
    let [(lines1, no_newline1), (lines2, no_newline2)] = sides;
    let first = fragments[0];
    let last = fragments[fragments.len() - 1];
    let context_start1 = first.start1.saturating_sub(context);
    let context_start2 = first.start2.saturating_sub(context);
    let context_end1 = (last.end1 + context).min(lines1.len());
    let context_end2 = (last.end2 + context).min(lines2.len());
    let line = |lines: &[&[u8]], index: usize, kind: LineKind, no_newline: bool| Line {
        kind,
        text: lines[index].to_vec(),
        no_newline: no_newline && index + 1 == lines.len(),
    };
    let mut result = Vec::new();
    let mut last_line1 = context_start1;
    for fragment in fragments {
        for index in last_line1..fragment.start1 {
            result.push(line(lines1, index, LineKind::Context, no_newline1));
        }
        for index in fragment.start1..fragment.end1 {
            result.push(line(lines1, index, LineKind::Removed, no_newline1));
        }
        for index in fragment.start2..fragment.end2 {
            result.push(line(lines2, index, LineKind::Added, no_newline2));
        }
        last_line1 = fragment.end1;
    }
    for index in last_line1..context_end1 {
        result.push(line(lines1, index, LineKind::Context, no_newline1));
    }
    Hunk {
        before_start: context_start1 + 1,
        before_count: context_end1 - context_start1,
        after_start: context_start2 + 1,
        after_count: context_end2 - context_start2,
        lines: result,
    }
}

// `TextPatchBuilder.compareLines`: a last line without a newline is compared on its own, so that
// `X` against `X\n` is a change of that line, as in git.
fn compare_lines(
    lines1: &[&[u8]],
    lines2: &[&[u8]],
    no_newline1: bool,
    no_newline2: bool,
) -> Vec<Range> {
    if !no_newline1 && !no_newline2 {
        return compare(lines1, lines2);
    }
    let last1 = lines1.len() - 1;
    let last2 = lines2.len() - 1;
    let compared1 = if no_newline1 {
        &lines1[..last1]
    } else {
        lines1
    };
    let compared2 = if no_newline2 {
        &lines2[..last2]
    } else {
        lines2
    };
    let ranges = compare(compared1, compared2);
    let change = if no_newline1 && no_newline2 {
        if lines1[last1] == lines2[last2] {
            return ranges;
        }
        Range {
            start1: last1,
            end1: last1 + 1,
            start2: last2,
            end2: last2 + 1,
        }
    } else if no_newline1 {
        Range {
            start1: last1,
            end1: last1 + 1,
            start2: last2 + 1,
            end2: last2 + 1,
        }
    } else {
        Range {
            start1: last1 + 1,
            end1: last1 + 1,
            start2: last2,
            end2: last2 + 1,
        }
    };
    append_range(ranges, change)
}

fn append_range(mut ranges: Vec<Range>, change: Range) -> Vec<Range> {
    match ranges.last_mut() {
        Some(last) if last.end1 == change.start1 && last.end2 == change.start2 => {
            last.end1 = change.end1;
            last.end2 = change.end2;
        }
        _ => ranges.push(change),
    }
    ranges
}

// IntelliJ's `ByLine.compare(…, DEFAULT)` is replaced by `similar`'s Myers, as in merge3.rs.
// Where two alignments are equally short, the hunks may differ from IntelliJ's; both apply.
// Past the deadline the result is still a valid diff, though not the shortest (IntelliJ then
// gives one range for the whole file).
fn compare(lines1: &[&[u8]], lines2: &[&[u8]]) -> Vec<Range> {
    let deadline = Instant::now() + DIFF_DEADLINE;
    let mut ranges = Vec::new();
    let mut current: Option<Range> = None;
    for op in capture_diff_slices_deadline(Algorithm::Myers, lines1, lines2, Some(deadline)) {
        let (tag, old, new) = op.as_tag_tuple();
        if tag == DiffTag::Equal {
            ranges.extend(current.take());
            continue;
        }
        match &mut current {
            Some(range) => {
                range.end1 = old.end;
                range.end2 = new.end;
            }
            None => {
                current = Some(Range {
                    start1: old.start,
                    end1: old.end,
                    start2: new.start,
                    end2: new.end,
                });
            }
        }
    }
    ranges.extend(current);
    ranges
}

// Git objects for the binary sections: `hash-object -w --no-filters`, and an attributes file that
// makes every blob binary for `git diff`, so that its output is always a `GIT binary patch`.
struct Blobs<'a> {
    repo: &'a Path,
    attributes: Option<tempfile::NamedTempFile>,
}

impl<'a> Blobs<'a> {
    fn new(repo: &'a Path) -> Self {
        Self {
            repo,
            attributes: None,
        }
    }

    fn hash(&self, content: &[u8]) -> Result<String, PatchError> {
        let output = git::run_with_input(
            self.repo,
            &["hash-object", "-w", "--stdin", "--no-filters"],
            content,
        )?;
        let hash = String::from_utf8_lossy(&output).trim().to_string();
        if hash.is_empty() || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(PatchError::Git(GitError::Io(format!(
                "git hash-object gave no hash: {hash}"
            ))));
        }
        Ok(hash)
    }

    fn diff(&mut self, from: &str, to: &str) -> Result<Vec<u8>, PatchError> {
        if self.attributes.is_none() {
            let mut file = tempfile::NamedTempFile::new()
                .map_err(|error| PatchError::Git(GitError::Io(error.to_string())))?;
            std::io::Write::write_all(&mut file, b"* binary\n")
                .map_err(|error| PatchError::Git(GitError::Io(error.to_string())))?;
            self.attributes = Some(file);
        }
        let attributes = self.attributes.as_ref().expect("just made");
        let config = format!("core.attributesFile={}", attributes.path().display());
        Ok(git::run(
            self.repo,
            &[
                "-c",
                &config,
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--binary",
                from,
                to,
            ],
        )?)
    }
}

// `TextPatchBuilder.buildBinaryPatch` with git's section (decision 3). The same content under the
// same name gives no patch; under another name, a rename without section. If git still gives a
// text diff (a repository attribute that wins over ours), the entry fails: as text, its CR bytes
// would become LF.
fn build_binary(
    repo: &Path,
    blobs: &mut Blobs,
    before: Option<Side>,
    after: Option<Side>,
) -> Result<Option<FilePatch>, PatchError> {
    if before.is_none() && after.is_none() {
        return Ok(None);
    }
    let before_hash = before
        .as_ref()
        .map(|side| blobs.hash(side.content))
        .transpose()?;
    let after_hash = after
        .as_ref()
        .map(|side| blobs.hash(side.content))
        .transpose()?;
    let zeros = zero_hash(before_hash.as_deref().or(after_hash.as_deref()));
    let same_name = matches!((&before, &after), (Some(one), Some(other)) if one.name == other.name);
    let git_section = if before_hash == after_hash {
        if same_name {
            return Ok(None);
        }
        Vec::new()
    } else {
        let empty = || blobs.hash(b"");
        let from = before_hash.clone().map_or_else(empty, Ok)?;
        let to = after_hash.clone().map_or_else(empty, Ok)?;
        let output = blobs.diff(&from, &to)?;
        match find(&output, b"\nGIT binary patch\n") {
            Some(offset) => output[offset + 1..].to_vec(),
            None => {
                let side = after
                    .as_ref()
                    .or(before.as_ref())
                    .expect("one side at least");
                return Err(PatchError::BinaryAsText {
                    path: side.path.to_string(),
                });
            }
        }
    };
    let new_mode = match (&before, &after) {
        (None, Some(after)) => Some(mode_on_disk(&repo.join(after.path))),
        _ => None,
    };
    Ok(Some(FilePatch::Binary(BinaryFilePatch {
        before: before.map(|side| side.name),
        after: after.map(|side| side.name),
        before_rev: Some(before_hash.unwrap_or_else(|| zeros.clone())),
        after_rev: Some(after_hash.unwrap_or(zeros)),
        new_mode,
        git_section,
    })))
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use regex::Regex;
    use tempfile::TempDir;

    use super::*;
    use crate::patch::test_helpers::*;
    use crate::test_support::{Random, TempRepo};

    // Before and after names, new mode and number of hunks.
    type Summary<'a> = (Option<&'a str>, Option<&'a str>, Option<u32>, usize);

    fn files_in(dir: &Path, prefix: &str) -> BTreeMap<String, Vec<u8>> {
        let Ok(entries) = fs::read_dir(dir) else {
            return BTreeMap::new();
        };
        entries
            .map(|entry| entry.unwrap())
            .map(|entry| {
                let name = entry.file_name().into_string().unwrap();
                (format!("{prefix}{name}"), fs::read(entry.path()).unwrap())
            })
            .collect()
    }

    fn masked(text: &str) -> String {
        let versions = Regex::new(r"\t\((?:revision|date) [^)\n]*\)").unwrap();
        versions.replace_all(text, "\t").into_owned()
    }

    fn dated(text: &str) -> String {
        let dates = Regex::new(r"\(date \d+\)").unwrap();
        dates.replace_all(text, "(date N)").into_owned()
    }

    // `PatchBuilderTest`: the files of `before/` and `after/` of each case, by name, with
    // `(revision before)` as the before revision. IntelliJ's test writes `(revision after)` for
    // the after one, and rama writes `(date <ms>)` of the file on disk (here there is none), so
    // the version ids are masked. `addFileForNullProject` builds with absolute paths, which end
    // up relative to the case folder; `contextLineCount` sets the registry to 5 lines;
    // `modifyWithCRLF` writes with CRLF there, and rama always with `\n`.
    #[test]
    fn writer_matches_intellij_fixtures() {
        let root = fixtures("patch");
        let repo = TempDir::new().unwrap();
        let options = options(repo.path());
        let cases = cases(&root);
        assert_eq!(cases.len(), 25);
        let mut failures = Vec::new();
        for case in &cases {
            let dir = root.join(case);
            let prefix = |side: &str| {
                if case == "addFileForNullProject" {
                    format!("{side}/")
                } else {
                    String::new()
                }
            };
            let before = files_in(&dir.join("before"), &prefix("before"));
            let after = files_in(&dir.join("after"), &prefix("after"));
            let short = |name: &String| name.rsplit('/').next().unwrap().to_string();
            let names: BTreeSet<String> = before.keys().chain(after.keys()).map(short).collect();
            let entries: Vec<PatchSource> = names
                .iter()
                .map(|name| {
                    let side = |files: &BTreeMap<String, Vec<u8>>, prefix: String| {
                        let path = format!("{prefix}{name}");
                        files.get(&path).map(|content| (path, content.clone()))
                    };
                    PatchSource {
                        before: side(&before, prefix("before")),
                        after: side(&after, prefix("after")),
                        before_rev: Some("before".to_string()),
                        binary: false,
                    }
                })
                .collect();
            let context = if case == "contextLineCount" {
                5
            } else {
                CONTEXT_LINES
            };
            let patch = build_with_context(repo.path(), &entries, &options, context).unwrap();
            let actual = String::from_utf8(write(&patch, &options)).unwrap();
            let expected = fs::read_to_string(dir.join("expected.patch"))
                .unwrap()
                .replace("\r\n", "\n");
            if masked(&actual) != masked(&expected) {
                failures.push(format!(
                    "{case}\n--- expected\n{expected}--- actual\n{actual}"
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    fn png(seed: u64, size: usize) -> Vec<u8> {
        let mut random = Random(seed);
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        bytes.extend((0..size).map(|_| random.below(256) as u8));
        bytes
    }

    fn git_apply(repo: &Path, patch: &[u8]) {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("p.patch");
        fs::write(&file, patch).unwrap();
        let output = crate::test_support::git_command()
            .current_dir(repo)
            .args(["apply", "--binary"])
            .arg(&file)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git apply: {}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(patch)
        );
    }

    // Decision 3: the sections come from `git diff --binary`, and `git apply --binary` applies
    // them (added, modified with a small change, deleted).
    #[test]
    fn binary_section_comes_from_git_and_reads_back() {
        let source = TempRepo::new();
        let target = TempRepo::new();
        let old = png(1, 3000);
        let mut changed = old.clone();
        changed[2000] ^= 0xff;
        target.commit("other.png", &old, "other");
        target.commit("old.png", &old, "old");
        let added = png(2, 500);
        let entries = [
            PatchSource {
                after: Some(("image.png".to_string(), added.clone())),
                binary: true,
                ..PatchSource::default()
            },
            PatchSource {
                before: Some(("other.png".to_string(), old.clone())),
                after: Some(("other.png".to_string(), changed.clone())),
                before_rev: Some("1".to_string()),
                binary: true,
            },
            PatchSource {
                before: Some(("old.png".to_string(), old.clone())),
                before_rev: Some("1".to_string()),
                binary: true,
                ..PatchSource::default()
            },
            // Unchanged: no patch.
            PatchSource {
                before: Some(("same.png".to_string(), old.clone())),
                after: Some(("same.png".to_string(), old.clone())),
                binary: true,
                ..PatchSource::default()
            },
        ];
        let options = options(source.path());
        let patch = build(source.path(), &entries, &options).unwrap();
        assert_eq!(patch.files.len(), 3);
        let written = write(&patch, &options);
        let text = String::from_utf8_lossy(&written).into_owned();
        let hash = |content: &[u8]| {
            let dir = TempDir::new().unwrap();
            fs::write(dir.path().join("f"), content).unwrap();
            source
                .git(&[
                    "hash-object",
                    "--no-filters",
                    dir.path().join("f").to_str().unwrap(),
                ])
                .trim()
                .to_string()
        };
        let zeros = "0".repeat(40);
        assert!(text.starts_with(&format!(
            "diff --git a/image.png b/image.png\nnew file mode 100644\nindex {zeros}..{}\nGIT binary patch\nliteral 516\n",
            hash(&added)
        )), "{text}");
        // A small change of a big file: git writes a delta, which git applies.
        assert!(
            text.contains(&format!(
                "diff --git a/other.png b/other.png\nindex {}..{}\nGIT binary patch\ndelta ",
                hash(&old),
                hash(&changed)
            )),
            "{text}"
        );
        assert!(text.contains(&format!(
            "diff --git a/old.png b/old.png\ndeleted file mode 100644\nindex {}..{zeros}\nGIT binary patch\nliteral 0\n",
            hash(&old)
        )));

        git_apply(target.path(), &written);
        assert_eq!(fs::read(target.path().join("image.png")).unwrap(), added);
        assert_eq!(fs::read(target.path().join("other.png")).unwrap(), changed);
        assert!(!target.path().join("old.png").exists());

        let read_back = read(&written).unwrap();
        assert_eq!(read_back.files, patch.files);
        let FilePatch::Binary(image) = &read_back.files[0] else {
            panic!("a text file patch");
        };
        assert_eq!(image.before, None);
        assert_eq!(image.after.as_deref(), Some("image.png"));
        assert!(image
            .git_section
            .starts_with(b"GIT binary patch\nliteral 516\n"));
        assert!(image.git_section.ends_with(b"\n\n"));
    }

    #[test]
    fn rename_new_deleted_and_mode_headers() {
        let repo = TempDir::new().unwrap();
        fs::create_dir(repo.path().join("a")).unwrap();
        for (name, content) in [
            ("renamed.txt", "a\nc\n"),
            ("moved.txt", "same\n"),
            ("a/empty.txt", ""),
            ("new.sh", "echo\n"),
        ] {
            fs::write(repo.path().join(name), content).unwrap();
        }
        let script = repo.path().join("new.sh");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let side =
            |path: &str, content: &str| Some((path.to_string(), content.as_bytes().to_vec()));
        let head = Some("abc123".to_string());
        let entries = [
            PatchSource {
                before: side("old.txt", "a\nb\n"),
                after: side("renamed.txt", "a\nc\n"),
                before_rev: head.clone(),
                binary: false,
            },
            PatchSource {
                before: side("from.txt", "same\n"),
                after: side("moved.txt", "same\n"),
                before_rev: head.clone(),
                binary: false,
            },
            PatchSource {
                after: side("a/empty.txt", ""),
                ..PatchSource::default()
            },
            PatchSource {
                after: side("new.sh", "echo\n"),
                ..PatchSource::default()
            },
            PatchSource {
                before: side("gone.txt", "x\n"),
                before_rev: head.clone(),
                ..PatchSource::default()
            },
            PatchSource {
                before: side("gone-empty.txt", ""),
                before_rev: head.clone(),
                ..PatchSource::default()
            },
        ];
        let mut options = options(repo.path());
        let patch = build(repo.path(), &entries, &options).unwrap();
        let written = String::from_utf8(write(&patch, &options)).unwrap();
        let separator = "===================================================================\n";
        let charset = concat!(
            "IDEA additional info:\n",
            "Subsystem: com.intellij.openapi.diff.impl.patch.CharsetEP\n",
            "<+>UTF-8\n",
        );
        let expected = [
            "Index: old.txt\n",
            charset,
            separator,
            "diff --git a/old.txt b/renamed.txt\n",
            "rename from old.txt\n",
            "rename to renamed.txt\n",
            "--- a/old.txt\t(revision abc123)\n",
            "+++ b/renamed.txt\t(date N)\n",
            "@@ -1,2 +1,2 @@\n",
            " a\n-b\n+c\n",
            "Index: new.sh\n",
            charset,
            separator,
            "diff --git a/new.sh b/new.sh\n",
            "new file mode 100755\n",
            "--- /dev/null\t(date N)\n",
            "+++ b/new.sh\t(date N)\n",
            "@@ -0,0 +1,1 @@\n",
            "+echo\n",
            "Index: gone.txt\n",
            separator,
            "diff --git a/gone.txt b/gone.txt\n",
            "deleted file mode 100644\n",
            "--- a/gone.txt\t(revision abc123)\n",
            "+++ /dev/null\t(revision abc123)\n",
            "@@ -1,1 +0,0 @@\n",
            "-x\n",
            "diff --git a/from.txt b/moved.txt\n",
            "rename from from.txt\n",
            "rename to moved.txt\n",
            "diff --git a/a/empty.txt b/a/empty.txt\n",
            "new file mode 100644\n",
            "diff --git a/gone-empty.txt b/gone-empty.txt\n",
            "deleted file mode 100644\n",
        ]
        .concat();
        assert_eq!(dated(&written), expected);

        let read_back = read(written.as_bytes()).unwrap();
        let names: Vec<Summary> = read_back
            .files
            .iter()
            .map(text)
            .map(|file| {
                (
                    file.before.as_deref(),
                    file.after.as_deref(),
                    file.new_mode,
                    file.hunks.len(),
                )
            })
            .collect();
        assert_eq!(
            names,
            [
                (Some("old.txt"), Some("renamed.txt"), None, 1),
                (None, Some("new.sh"), Some(0o100755), 1),
                (Some("gone.txt"), None, None, 1),
                (Some("from.txt"), Some("moved.txt"), None, 0),
                (None, Some("a/empty.txt"), Some(0o100644), 0),
                (Some("gone-empty.txt"), None, None, 0),
            ]
        );

        // Reverse: the deletion becomes a creation, with the version ids of the old side.
        options.reverse = true;
        let reversed = build(repo.path(), &entries[4..5], &options).unwrap();
        assert_eq!(
            String::from_utf8(write(&reversed, &options)).unwrap(),
            [
                "Index: gone.txt\n",
                separator,
                "diff --git a/gone.txt b/gone.txt\n",
                "new file mode 100644\n",
                "--- /dev/null\t(revision abc123)\n",
                "+++ b/gone.txt\t(revision abc123)\n",
                "@@ -0,0 +1,1 @@\n",
                "+x\n",
            ]
            .concat()
        );

        // The base path: names relative to it.
        let mut options = self::options(&repo.path().join("sub"));
        options.reverse = false;
        let nested = build(
            repo.path(),
            &[modified("sub/x.txt", b"a\n", b"b\n")],
            &options,
        )
        .unwrap();
        assert_eq!(text(&nested.files[0]).before.as_deref(), Some("x.txt"));

        // Git headers from git: mode only, mode of a new file, deletion, rename, `index`.
        let mode_only = read(
            &fs::read(fixtures(
                "patchReader/patchWithFileModeChangedOnly/test.patch",
            ))
            .unwrap(),
        )
        .unwrap();
        let file = text(&mode_only.files[0]);
        assert_eq!(
            (file.before.as_deref(), file.after.as_deref()),
            (Some("l.php"), Some("l.php"))
        );
        assert_eq!(file.new_mode, Some(0o100644));
        assert!(file.hunks.is_empty());
        let from_git = concat!(
            "diff --git a/x.txt b/y.txt\n",
            "similarity index 90%\n",
            "rename from x.txt\n",
            "rename to y.txt\n",
            "index 1234567..89abcde 100644\n",
            "--- a/x.txt\n",
            "+++ b/y.txt\n",
            "@@ -1 +1 @@\n",
            "-a\n",
            "+b\n",
            "diff --git a/n.sh b/n.sh\n",
            "new file mode 100755\n",
            "index 0000000..e69de29\n",
            "diff --git a/d.txt b/d.txt\n",
            "deleted file mode 100644\n",
            "index e69de29..0000000\n",
        );
        let read_back = read(from_git.as_bytes()).unwrap();
        let files: Vec<&TextFilePatch> = read_back.files.iter().map(text).collect();
        assert_eq!(
            (files[0].before.as_deref(), files[0].after.as_deref()),
            (Some("x.txt"), Some("y.txt"))
        );
        assert_eq!(files[0].before_rev.as_deref(), Some("1234567"));
        assert_eq!(files[0].after_rev.as_deref(), Some("89abcde"));
        let hunk = &files[0].hunks[0];
        assert_eq!(
            (
                hunk.before_start,
                hunk.before_count,
                hunk.after_start,
                hunk.after_count
            ),
            (1, 1, 1, 1)
        );
        assert_eq!(
            (files[1].before.as_deref(), files[1].new_mode),
            (None, Some(0o100755))
        );
        assert_eq!((files[2].after.as_deref(), files[2].new_mode), (None, None));

        // Quoted names (git's octal bytes and escapes) are read; names that the reader would take
        // apart (`\`, `"`, control characters) are written quoted, the rest raw.
        let quoted = concat!(
            "diff --git \"a/t\\303\\251st \\\"q\\\".txt\" \"b/t\\303\\251st \\\"q\\\".txt\"\n",
            "--- \"a/t\\303\\251st \\\"q\\\".txt\"\t\n",
            "+++ \"b/t\\303\\251st \\\"q\\\".txt\"\t\n",
            "@@ -1 +1 @@\n-a\n+b\n",
        );
        let read_back = read(quoted.as_bytes()).unwrap();
        assert_eq!(
            text(&read_back.files[0]).before.as_deref(),
            Some("tést \"q\".txt")
        );
        let options = self::options(repo.path());
        for name in [
            "dir/tést file.txt",
            "back\\slash \"q\".txt",
            "tab\there.txt",
        ] {
            let patch = build(repo.path(), &[modified(name, b"a\n", b"b\n")], &options).unwrap();
            let written = write(&patch, &options);
            let read_back = read(&written).unwrap();
            let file = text(&read_back.files[0]);
            assert_eq!(
                (file.before.as_deref(), file.after.as_deref()),
                (Some(name), Some(name))
            );
            let written = String::from_utf8(written).unwrap();
            if name.starts_with("dir/") {
                assert!(written.contains("diff --git a/dir/tést file.txt b/dir/tést file.txt\n"));
            }
            if name.starts_with("back") {
                assert!(
                    written.contains(r#"--- "a/back\\slash \"q\".txt""#),
                    "{written}"
                );
            }
        }
    }

    #[test]
    fn cr_never_reaches_hunk_lines() {
        let repo = TempDir::new().unwrap();
        let options = options(repo.path());
        let entries = [modified(
            "f.txt",
            b"one\r\ntwo\rthree\r\n",
            b"one\r\nTWO\r\nthree",
        )];
        let patch = build(repo.path(), &entries, &options).unwrap();
        let written = write(&patch, &options);
        assert!(!written.contains(&b'\r'));
        assert_eq!(
            String::from_utf8(written).unwrap(),
            concat!(
                "Index: f.txt\n",
                "===================================================================\n",
                "diff --git a/f.txt b/f.txt\n",
                "--- a/f.txt\t(revision 1)\n",
                "+++ b/f.txt\t\n",
                "@@ -1,3 +1,3 @@\n",
                " one\n",
                "-two\n",
                "-three\n",
                "+TWO\n",
                "+three\n",
                "\\ No newline at end of file\n",
            )
        );

        // A patch with CRLF reads as the same patch with LF, and a lone CR splits a line.
        let crlf = fs::read(fixtures("patch/modifyWithCRLF/expected.patch")).unwrap();
        assert!(crlf.contains(&b'\r'));
        let lf: Vec<u8> = String::from_utf8(crlf.clone())
            .unwrap()
            .replace("\r\n", "\n")
            .into_bytes();
        let from_crlf = read(&crlf).unwrap();
        assert_eq!(from_crlf, read(&lf).unwrap());
        for line in &text(&from_crlf.files[0]).hunks[0].lines {
            assert!(!line.text.contains(&b'\r'));
        }
        let lone = read(b"--- a/f\r+++ b/f\r@@ -1,2 +1,2 @@\r a\r-b\r+c\r").unwrap();
        assert_eq!(text(&lone.files[0]).hunks[0].lines.len(), 3);
    }

    // The test's own exact applier: each hunk at its stated line, every context and removed line
    // checked, `\ No newline at end of file` included.
    fn apply_exact(text: &[u8], hunks: &[Hunk]) -> Vec<u8> {
        let mut lines: Vec<(&[u8], bool)> = Vec::new();
        let mut rest = text;
        while !rest.is_empty() {
            match rest.iter().position(|byte| *byte == b'\n') {
                Some(end) => {
                    lines.push((&rest[..end], true));
                    rest = &rest[end + 1..];
                }
                None => {
                    lines.push((rest, false));
                    rest = &[];
                }
            }
        }
        let mut result: Vec<(Vec<u8>, bool)> = Vec::new();
        let mut cursor = 0;
        for hunk in hunks {
            let start = hunk.before_start.saturating_sub(1);
            assert!(start >= cursor);
            result.extend(
                lines[cursor..start]
                    .iter()
                    .map(|(line, newline)| (line.to_vec(), *newline)),
            );
            cursor = start;
            for line in &hunk.lines {
                match line.kind {
                    LineKind::Added => result.push((line.text.clone(), !line.no_newline)),
                    LineKind::Context | LineKind::Removed => {
                        assert_eq!(lines[cursor], (&line.text[..], !line.no_newline));
                        if line.kind == LineKind::Context {
                            result.push((line.text.clone(), !line.no_newline));
                        }
                        cursor += 1;
                    }
                }
            }
        }
        result.extend(
            lines[cursor..]
                .iter()
                .map(|(line, newline)| (line.to_vec(), *newline)),
        );
        let mut out = Vec::new();
        for (line, newline) in result {
            out.extend(line);
            if newline {
                out.push(b'\n');
            }
        }
        out
    }

    fn random_text(random: &mut Random) -> Vec<u8> {
        const LINES: [&str; 12] = [
            "",
            "a",
            "b",
            "c",
            "a b",
            " ",
            // Content that looks like patch syntax, as in `applyPatch/wrongFileStartUnified`.
            "-- x",
            "++ x",
            "@@ -1 +1 @@",
            "\\ No newline at end of file",
            "diff --git a b",
            "IDEA additional info:",
        ];
        let count = random.below(12);
        let mut text: Vec<u8> = Vec::new();
        for index in 0..count {
            text.extend(LINES[random.below(LINES.len() as u32) as usize].as_bytes());
            if index + 1 < count || random.below(2) == 0 {
                text.push(b'\n');
            }
        }
        text
    }

    fn check_symmetry(repo: &Path, before: Option<&[u8]>, after: Option<&[u8]>) {
        let path = "some/file.txt".to_string();
        let source = PatchSource {
            before: before.map(|content| (path.clone(), content.to_vec())),
            after: after.map(|content| (path.clone(), content.to_vec())),
            before_rev: Some("1".to_string()),
            binary: false,
        };
        let options = options(repo);
        let built = build(repo, &[source], &options).unwrap();
        if before == after {
            assert!(built.files.is_empty());
            return;
        }
        let written = write(&built, &options);
        let read_back = read(&written).unwrap();
        // A git patch without `index` keeps no version ids.
        let mut expected = built.clone();
        for file in &mut expected.files {
            if let FilePatch::Text(file) = file {
                file.before_rev = None;
                file.after_rev = None;
            }
        }
        assert_eq!(read_back, expected, "{}", String::from_utf8_lossy(&written));
        let file = text(&read_back.files[0]);
        let patched = apply_exact(before.unwrap_or_default(), &file.hunks);
        assert_eq!(
            patched,
            after.unwrap_or_default(),
            "{}",
            String::from_utf8_lossy(&written)
        );
    }

    // `CreateApplyPatchSymmetryTest`: create, write, read and apply gives the after text, and the
    // patch read is the one built. Its whole-file and line-ending cases, then 2,000 seeded random
    // pairs (some of them creations and deletions).
    #[test]
    fn property_create_then_read_is_identity() {
        let repo = TempDir::new().unwrap();
        let repo = repo.path();
        for (before, after) in [("x", ""), ("x\n", ""), ("x\ny", ""), ("x\ny\n", "")] {
            check_symmetry(repo, Some(before.as_bytes()), Some(after.as_bytes()));
            check_symmetry(repo, Some(after.as_bytes()), Some(before.as_bytes()));
        }
        let unchanged = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n13\n";
        let endings = [
            ("x", "x\n"),
            ("x", "x\ny"),
            ("x", "x\ny\n"),
            ("x\n", "x\ny"),
            ("x\n", "x\ny\n"),
        ];
        for (before, after) in endings {
            for prefix in ["", unchanged] {
                let before = format!("{prefix}{before}");
                let after = format!("{prefix}{after}");
                check_symmetry(repo, Some(before.as_bytes()), Some(after.as_bytes()));
                check_symmetry(repo, Some(after.as_bytes()), Some(before.as_bytes()));
            }
        }
        let mut random = Random(20261108);
        for _ in 0..2000 {
            let before = random_text(&mut random);
            let after = random_text(&mut random);
            match random.below(10) {
                0 => check_symmetry(repo, None, Some(&after)),
                1 => check_symmetry(repo, Some(&before), None),
                _ => check_symmetry(repo, Some(&before), Some(&after)),
            }
        }
    }

    // The base of a CRLF file is kept with `\n`, as reading gives it back.
    #[test]
    fn a_crlf_base_reads_back_as_built() {
        let repo = TempDir::new().unwrap();
        fs::write(repo.path().join("f.txt"), "a\r\nc\r\n").unwrap();
        let mut options = options(repo.path());
        options.include_base = true;
        let entries = [modified("f.txt", b"a\r\nb\r\n", b"a\r\nc\r\n")];
        let built = build(repo.path(), &entries, &options).unwrap();
        assert_eq!(text_of(&built).base.as_deref(), Some(&b"a\nb\n"[..]));
        let read_back = read(&write(&built, &options)).unwrap();
        assert_eq!(text_of(&read_back).base, text_of(&built).base);
    }

    // A repository attribute can make git print a text diff for a binary entry: the entry fails, and
    // its bytes are never changed (a text patch would turn its CR bytes into LF).
    #[test]
    fn a_binary_entry_that_git_diffs_as_text_fails() {
        let repo = TempRepo::new();
        fs::write(repo.path().join(".git/info/attributes"), "* diff\n").unwrap();
        let entries = [PatchSource {
            before: Some(("data.bin".to_string(), b"one\r\ntwo\0\r\n".to_vec())),
            after: Some(("data.bin".to_string(), b"one\r\nthree\0\r\n".to_vec())),
            before_rev: Some("1".to_string()),
            binary: true,
        }];
        assert_eq!(
            build(repo.path(), &entries, &options(repo.path())),
            Err(PatchError::BinaryAsText {
                path: "data.bin".to_string()
            })
        );
    }
}
