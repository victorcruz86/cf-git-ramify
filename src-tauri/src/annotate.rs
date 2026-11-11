// Annotate with Git Blame (`GitAnnotationProvider`, `GitFileAnnotation`, `AnnotateToggleAction`), in
// the viewer (decision 7 of phase 4 part 4), with 241's defaults.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State, WebviewWindow};

use std::collections::HashMap;

use crate::commands::{
    blocking_api, current_repo, failed, owner_label, try_emit_to_owner, ApiError, AppState,
};
use crate::diff::check_hash;
use crate::git::{self, GitError};
use crate::history::{self, Git, Starts, Walk};
use crate::hunks;
use crate::task::{TaskId, TaskKind};

// `retrieving.annotations` (`V:538`).
pub const LOADING_ANNOTATIONS: &str = "Loading Annotations";
// The main window selects the commit in its Log (`vcs.blame.show.affected.files.in.log`).
const SHOW_IN_LOG_EVENT: &str = "annotation-show-in-log";
const HEAD: &str = "HEAD";

// A file in a commit: `GitFileRevision`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRevision {
    pub hash: String,
    pub path: String,
}

// `GitFileAnnotation.CommitInfo`, with what the gutter shows of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotatedCommit {
    pub hash: String,
    // The file's path in this commit.
    pub path: String,
    pub author: String,
    pub email: String,
    // Seconds.
    pub author_time: i64,
    pub committer_time: i64,
    pub subject: String,
    pub message: String,
    // `Annotate Previous Revision`: blame's `previous`, else the next revision of the history.
    pub previous: Option<FileRevision>,
    // `HistoryIdColumn`: its place in the file history, the oldest is 1.
    pub number: Option<u32>,
    // The background in the `Order` and `Author` colour modes.
    pub order_color: Option<String>,
    pub author_color: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Annotation {
    // `getCurrentRevision`: the revision blamed, and the file's path in it.
    pub revision: String,
    pub path: String,
    pub commits: Vec<AnnotatedCommit>,
    // For each line of the text shown, from the first, its commit in `commits`; `None` for a line
    // changed locally.
    pub lines: Vec<Option<u32>>,
    // For each line shown, its line in the revision blamed, from 0: where a changed line's change
    // starts there (`transferLineToVcs(line, approximate = true)`).
    pub origins: Vec<u32>,
    // The commit of each line of the revision blamed: a new text of the local file is mapped again
    // from these, without a new blame.
    pub owners: Vec<u32>,
    // `AnnotateToggleAction.doAnnotate`: the lines of the revision blamed as a document counts them
    // (`\r\n`, `\r` and `\n` end a line) differ from blame's by more than one (`V:687`).
    pub suspicious: bool,
    // The size of the history the numbers come from, 0 without numbers.
    pub history_size: u32,
    // The line (from 1) of `from`, translated into this revision.
    pub line: Option<u32>,
}

// The line (from 1) of the text an annotation was asked from: `Annotate Revision` and `Annotate
// Previous Revision` open the new one at the same line.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FromLine {
    pub text: String,
    pub line: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AnnotateRequest {
    pub path: String,
    // A full hash; without it, the local file.
    pub revision: Option<String>,
    // The text the viewer shows of the local file, whose lines are mapped to the blamed ones.
    pub local_text: Option<String>,
    pub from: Option<FromLine>,
}

// `GitAnnotationProvider.annotate`: the blame of the revision, or for the local file the blame of
// the last commit that touched it, its lines mapped to the text shown (`UpToDateLineNumberProvider`).
// Then the file history from there (`loadFileHistoryInBackground`), for the numbers and `Annotate
// Previous Revision`, and the full messages for the tooltip.
pub fn annotate(git: &mut Git, request: &AnnotateRequest) -> Result<Annotation, GitError> {
    let (path, revision, start) = match &request.revision {
        Some(revision) => {
            check_hash(revision)?;
            (request.path.clone(), revision.clone(), revision.clone())
        }
        None => {
            let path = history::last_commit_path(git, &request.path);
            let revision = last_revision(git, &path)?;
            (path, revision, HEAD.to_string())
        }
    };
    let blamed = parse_porcelain(&git(&blame_args(&revision, &path))?)?;
    let mut commits = blamed.commits;
    let starts = Starts::Revisions(vec![start]);
    let rows = history::file_history(git, &path, &starts, Walk::FirstAddition)?;
    let numbered = rows.len() >= 2;
    for commit in &mut commits {
        let at = rows.iter().position(|row| row.hash == commit.hash);
        if numbered {
            commit.number = at.map(|index| (rows.len() - index) as u32);
        }
        if commit.previous.is_none() {
            commit.previous = at
                .and_then(|index| rows.get(index + 1))
                .map(|row| FileRevision {
                    hash: row.hash.clone(),
                    path: row.path.clone(),
                });
        }
    }
    let messages = full_messages(git, &commits)?;
    for commit in &mut commits {
        if let Some(message) = messages.get(&commit.hash) {
            commit.message = message.clone();
        }
    }
    color_by_order(&mut commits);
    color_by_author(&mut commits);
    let base: Vec<&str> = blamed.lines.iter().map(|(_, text)| text.as_str()).collect();
    let owners: Vec<u32> = blamed.lines.iter().map(|(commit, _)| *commit).collect();
    let (lines, origins) = match &request.local_text {
        Some(text) => shown_lines(&owners, &base, &lines_of(text)),
        None => (
            owners.iter().copied().map(Some).collect(),
            (0..owners.len() as u32).collect(),
        ),
    };
    let expected = document_lines(&blob_text(git, &revision, &path)?);
    let suspicious = expected.abs_diff(base.len().max(1)) > 1;
    let line = request
        .from
        .as_ref()
        .map(|from| translated(&lines_of(&from.text), &base, from.line));
    Ok(Annotation {
        revision,
        path,
        commits,
        lines,
        origins,
        owners,
        suspicious,
        history_size: if numbered { rows.len() as u32 } else { 0 },
        line,
    })
}

// What a change of the repository or of the local file does to the annotation of a local file
// (`VcsAnnotationLocalChangesListenerImpl.processFile`, 241:145-160): the base revision moved
// (`GitFileAnnotation.isBaseRevisionChanged`, 241:410-414) closes it (`invalidateAnnotations(..,
// false)` → `FileAnnotation.close()`, 189-198); else only the lines of the new text are mapped
// again (`UpToDateLineNumberProviderImpl`), with the commits it has.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RefreshRequest {
    // The local file.
    pub path: String,
    // The annotation's `revision`, `path` and `owners`.
    pub revision: String,
    pub blamed_path: String,
    pub owners: Vec<u32>,
    pub local_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Refreshed {
    Closed,
    Lines {
        lines: Vec<Option<u32>>,
        origins: Vec<u32>,
    },
}

pub fn refresh(git: &mut Git, request: &RefreshRequest) -> Result<Refreshed, GitError> {
    check_hash(&request.revision)?;
    // `GitDiffProvider.getCurrentRevision` = `GitHistoryUtils.getCurrentRevision(.., "HEAD")`.
    let path = history::last_commit_path(git, &request.path);
    if last_revision(git, &path)? != request.revision {
        return Ok(Refreshed::Closed);
    }
    let text = blob_text(git, &request.revision, &request.blamed_path)?;
    let base = blamed_lines(&text);
    if base.len() != request.owners.len() {
        return Ok(Refreshed::Closed);
    }
    let (lines, origins) = shown_lines(&request.owners, &base, &lines_of(&request.local_text));
    Ok(Refreshed::Lines { lines, origins })
}

// The blob of `<revision>:<path>`, with the exact bytes of the name, as blame reads it.
fn blob_text(git: &mut Git, revision: &str, path: &str) -> Result<String, GitError> {
    let object = format!("{revision}:{path}");
    git(&git::exact(&["cat-file", "blob", &object]))
}

// The lines of a blob as `git blame` gives them: split at `\n`, without the `\r` of a `\r\n` and
// without the byte order mark the viewer does not show.
fn blamed_lines(text: &str) -> Vec<&str> {
    let mut lines = lines_of(text);
    if let Some(first) = lines.first_mut() {
        *first = first.strip_prefix('\u{feff}').unwrap_or(first);
    }
    lines
}

// `Document.getLineCount`: every `\r\n`, `\r` or `\n` ends a line, and the last one starts another.
fn document_lines(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut lines = 1;
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'\r' if bytes.get(at + 1) == Some(&b'\n') => {
                lines += 1;
                at += 1;
            }
            b'\r' | b'\n' => lines += 1,
            _ => {}
        }
        at += 1;
    }
    lines
}

// `GitAnnotationService.annotate` with `ANNOTATE_IGNORE_SPACES` on and `ANNOTATE_DETECT_INNER_MOVEMENTS`
// `NONE` (`GitVcsApplicationSettings`): `-w`, no `-M` or `-C`. The revision is a full hash or `HEAD`.
fn blame_args<'a>(revision: &'a str, path: &'a str) -> Vec<&'a str> {
    git::exact(&[
        "blame",
        "--porcelain",
        "-l",
        "-t",
        "--encoding=UTF-8",
        "-w",
        revision,
        "--",
        path,
    ])
}

// `GitHistoryUtils.getCurrentRevision(.., HEAD)`: the last commit of HEAD that touched the file.
// Without one, 241 blames `HEAD` and git says why it cannot.
fn last_revision(git: &mut Git, path: &str) -> Result<String, GitError> {
    // `annotate_refresh` runs it without a task, and so without the task's options.
    let args = git::exact(&[
        history::NO_SIGNATURES[0],
        history::NO_SIGNATURES[1],
        "--literal-pathspecs",
        "log",
        "-n1",
        "--format=%H",
        "--end-of-options",
        HEAD,
        "--",
        path,
    ]);
    let output = git(&args)?;
    let hash = output.trim();
    Ok(if hash.is_empty() { HEAD } else { hash }.to_string())
}

struct Blamed {
    commits: Vec<AnnotatedCommit>,
    // Each line of the blamed revision: its commit in `commits`, and its text.
    lines: Vec<(u32, String)>,
}

// `GitAnnotationProvider.parseAnnotations`: a commit's data comes with its first line only.
fn parse_porcelain(output: &str) -> Result<Blamed, GitError> {
    let mut commits: Vec<AnnotatedCommit> = Vec::new();
    let mut known: HashMap<String, u32> = HashMap::new();
    let mut lines = Vec::new();
    let mut rows = output.split('\n');
    while let Some(header) = rows.next() {
        if header.is_empty() {
            continue;
        }
        let mut fields = header.split(' ');
        let hash = fields.next().unwrap_or_default().to_string();
        let number: usize = fields
            .nth(1)
            .and_then(|field| field.parse().ok())
            .unwrap_or(0);
        let expected = lines.len() + 1;
        if number != expected {
            // `annotate.line.mismatch.exception` (`B:20`).
            return Err(GitError::Io(format!(
                "Adding info for line {number}, but we are expecting it to be for line {expected}"
            )));
        }
        let mut info = (!known.contains_key(&hash)).then(CommitInfo::default);
        let text = loop {
            let row = rows.next().ok_or_else(|| lacks_data(number))?;
            if let Some(text) = row.strip_prefix('\t') {
                // A `\r\n` file keeps its `\r` in blame's text: the line is compared without it.
                break text.strip_suffix('\r').unwrap_or(text).to_string();
            }
            if let Some(info) = info.as_mut() {
                info.read(row);
            }
        };
        if let Some(info) = info {
            let commit = info.into_commit(&hash).ok_or_else(|| lacks_data(number))?;
            known.insert(hash.clone(), commits.len() as u32);
            commits.push(commit);
        }
        lines.push((known[&hash], text));
    }
    if let Some((_, first)) = lines.first_mut() {
        // The viewer shows the text without its byte order mark.
        if let Some(rest) = first.strip_prefix('\u{feff}') {
            *first = rest.to_string();
        }
    }
    Ok(Blamed { commits, lines })
}

// `annotate.output.lack.data` (`B:19`).
fn lacks_data(line: usize) -> GitError {
    GitError::Io(format!("Output for line {line} lacks the necessary data"))
}

#[derive(Default)]
struct CommitInfo {
    author: Option<String>,
    email: Option<String>,
    author_time: Option<i64>,
    committer_time: Option<i64>,
    subject: Option<String>,
    path: Option<String>,
    previous: Option<FileRevision>,
}

impl CommitInfo {
    fn read(&mut self, row: &str) {
        let (key, value) = row.split_once(' ').unwrap_or((row, ""));
        match key {
            "author" => self.author = Some(value.to_string()),
            "author-mail" => {
                let email = value
                    .strip_prefix('<')
                    .and_then(|rest| rest.strip_suffix('>'));
                self.email = Some(email.unwrap_or(value).to_string());
            }
            "author-time" => self.author_time = value.parse().ok(),
            "committer-time" => self.committer_time = value.parse().ok(),
            "summary" => self.subject = Some(value.to_string()),
            "filename" => self.path = Some(unquoted(value)),
            "previous" => {
                self.previous = value.split_once(' ').map(|(hash, path)| FileRevision {
                    hash: hash.to_string(),
                    path: unquoted(path),
                });
            }
            _ => {}
        }
    }

    fn into_commit(self, hash: &str) -> Option<AnnotatedCommit> {
        let subject = self.subject?;
        Some(AnnotatedCommit {
            hash: hash.to_string(),
            path: self.path?,
            author: self.author?,
            email: self.email?,
            author_time: self.author_time?,
            committer_time: self.committer_time?,
            // Until the full message is read (`subject + "\n..."` in 241).
            message: format!("{subject}\n..."),
            subject,
            previous: self.previous,
            number: None,
            order_color: None,
            author_color: None,
        })
    }
}

// `quote_c_style`: git quotes a name with a `"`, a `\`, a control character or (with
// `core.quotePath`) a byte above 0x7f.
fn unquoted(name: &str) -> String {
    let Some(inner) = name
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    else {
        return name.to_string();
    };
    let bytes = inner.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] != b'\\' {
            out.push(bytes[at]);
            at += 1;
            continue;
        }
        let escaped = match bytes.get(at + 1) {
            Some(b'a') => Some(0x07),
            Some(b'b') => Some(0x08),
            Some(b't') => Some(b'\t'),
            Some(b'n') => Some(b'\n'),
            Some(b'v') => Some(0x0b),
            Some(b'f') => Some(0x0c),
            Some(b'r') => Some(b'\r'),
            Some(b'"') => Some(b'"'),
            Some(b'\\') => Some(b'\\'),
            _ => None,
        };
        if let Some(byte) = escaped {
            out.push(byte);
            at += 2;
            continue;
        }
        let digits = bytes
            .get(at + 1..at + 4)
            .filter(|digits| digits.iter().all(|digit| (b'0'..=b'7').contains(digit)));
        match digits {
            Some(digits) => {
                let value = digits
                    .iter()
                    .fold(0u32, |value, digit| value * 8 + u32::from(digit - b'0'));
                out.push(value as u8);
                at += 4;
            }
            None => {
                out.push(b'\\');
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// `GitFileRevision.getCommitMessage`: the full messages, trimmed, read once for all the commits.
fn full_messages(
    git: &mut Git,
    commits: &[AnnotatedCommit],
) -> Result<HashMap<String, String>, GitError> {
    let mut messages = HashMap::new();
    for chunk in commits.chunks(git::PATHS_PER_COMMAND) {
        let mut args = vec![
            "log",
            "--no-walk=unsorted",
            "--format=%x01%H%x00%B",
            "--encoding=UTF-8",
            "--end-of-options",
        ];
        args.extend(chunk.iter().map(|commit| commit.hash.as_str()));
        for record in git(&args)?.split('\u{1}') {
            if let Some((hash, message)) = record.split_once('\0') {
                messages.insert(hash.to_string(), message.trim().to_string());
            }
        }
    }
    Ok(messages)
}

// `GitRevisionsOrderProvider` and `AnnotateToggleAction.computeBgColors`: the revisions grouped by
// committer date, newest first; group `i` of `n` takes `palette[21 * i / n]`. A single revision has
// no background.
fn color_by_order(commits: &mut [AnnotatedCommit]) {
    if commits.len() < 2 {
        return;
    }
    let mut dates: Vec<i64> = commits.iter().map(|commit| commit.committer_time).collect();
    dates.sort_unstable_by(|a, b| b.cmp(a));
    dates.dedup();
    for commit in commits {
        let group = dates
            .iter()
            .position(|date| *date == commit.committer_time)
            .unwrap_or(0);
        commit.order_color =
            Some(ORDER_COLORS[ORDER_COLORS.len() * group / dates.len()].to_string());
    }
}

// `GitAuthorsMappingProvider`: the author names sorted, each with `palette[k % 21]` of the palette
// reordered for authors.
fn color_by_author(commits: &mut [AnnotatedCommit]) {
    if commits.len() < 2 {
        return;
    }
    let mut authors: Vec<String> = commits.iter().map(|commit| commit.author.clone()).collect();
    authors.sort_unstable();
    authors.dedup();
    for commit in commits {
        let index = authors.binary_search(&commit.author).unwrap_or(0);
        commit.author_color = Some(AUTHOR_COLORS[index % AUTHOR_COLORS.len()].to_string());
    }
}

// The lines of a text as the editor numbers them: a `\r` before the `\n` is not part of the line,
// and a last `\n` starts no line of its own.
fn lines_of(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

fn joined<S: AsRef<str>>(lines: &[S]) -> String {
    lines
        .iter()
        .flat_map(|line| [line.as_ref(), "\n"])
        .collect()
}

// `UpToDateLineNumberProviderImpl` with `AnnotationGutterLineConvertorProxy`: a line inserted or
// changed since the blamed revision has no annotation; the others keep their commit. With each
// line, where it is in the blamed revision: a changed one where its change starts there.
fn shown_lines(owners: &[u32], base: &[&str], shown: &[&str]) -> (Vec<Option<u32>>, Vec<u32>) {
    let mut lines = Vec::with_capacity(shown.len());
    let mut origins = Vec::with_capacity(shown.len());
    let mut old = 0usize;
    for hunk in hunks::compute(joined(base).as_bytes(), joined(shown).as_bytes()) {
        while lines.len() < hunk.new_start as usize {
            lines.push(owners.get(old).copied());
            origins.push(old as u32);
            old += 1;
        }
        lines.extend(std::iter::repeat_n(None, hunk.new_lines as usize));
        origins.extend(std::iter::repeat_n(hunk.old_start, hunk.new_lines as usize));
        old = (hunk.old_start + hunk.old_lines) as usize;
    }
    while lines.len() < shown.len() {
        lines.push(owners.get(old).copied());
        origins.push(old as u32);
        old += 1;
    }
    (lines, origins)
}

// `Diff.translateLine(before, after, line, approximate = true)`: lines compared trimmed (`String.trim`);
// a line inside a change goes to where the change starts in `after`. From 1.
fn translated(before: &[&str], after: &[&str], line: u32) -> u32 {
    let trim = |lines: &[&str]| {
        let trimmed: Vec<&str> = lines
            .iter()
            .map(|line| line.trim_matches(|c: char| c <= ' '))
            .collect();
        joined(&trimmed)
    };
    let target = line.saturating_sub(1);
    let mut result = i64::from(target);
    for hunk in hunks::compute(trim(before).as_bytes(), trim(after).as_bytes()) {
        if target < hunk.old_start {
            break;
        }
        if target < hunk.old_start + hunk.old_lines {
            return hunk.new_start + 1;
        }
        result += i64::from(hunk.new_lines) - i64::from(hunk.old_lines);
    }
    result.max(0) as u32 + 1
}

// The 21 colours of `AnnotationsSettings` for the dark scheme (G§10): five anchors
// (`VCS_ANNOTATIONS_COLOR_1..5`, `expUI_darkScheme.xml:46-50`) with four steps between each two.
pub const ORDER_COLORS: [&str; 21] = [
    "#2f5194", "#2e4d8b", "#2e4a83", "#2d477b", "#2d4473", "#2d416b", "#2c3f66", "#2b3d61",
    "#2a3b5d", "#293958", "#283754", "#273550", "#26334c", "#253149", "#242f45", "#242d42",
    "#232b3e", "#23293a", "#222836", "#222632", "#22252e",
];
// `AnnotationsSettings.getAuthorsColors`: the same colours taken with a stride of 4.
pub const AUTHOR_COLORS: [&str; 21] = [
    "#2f5194", "#2d4473", "#2a3b5d", "#26334c", "#232b3e", "#22252e", "#2e4d8b", "#2d416b",
    "#293958", "#253149", "#23293a", "#2e4a83", "#2c3f66", "#283754", "#242f45", "#222836",
    "#2d477b", "#2b3d61", "#273550", "#242d42", "#222632",
];

// `Diff.translateLine` of two texts, from 1.
pub fn translate_line(before: &str, after: &str, line: u32) -> u32 {
    translated(&lines_of(before), &lines_of(after), line)
}

#[tauri::command]
pub async fn annotate_start(
    request: AnnotateRequest,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    let repo = current_repo(&state, &window)?;
    let title = LOADING_ANNOTATIONS.to_string();
    Ok(state.tasks.start(
        &owner_label(&window),
        &repo,
        TaskKind::Read,
        title,
        move |context| {
            let mut git = |args: &[&str]| context.git(args).map(|output| output.stdout);
            annotate(&mut git, &request)
        },
    ))
}

// The local file changed on disk or the repository did: closed, or the lines mapped again.
#[tauri::command]
pub async fn annotate_refresh(
    request: RefreshRequest,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Refreshed, ApiError> {
    let repo = current_repo(&state, &window)?;
    blocking_api(move || {
        let mut git = |args: &[&str]| {
            git::run(&repo, args).map(|output| String::from_utf8_lossy(&output).into_owned())
        };
        Ok(refresh(&mut git, &request)?)
    })
    .await
}

// A click on an annotated line: its commit in the main window's Log.
#[tauri::command]
pub fn annotation_show_in_log(
    hash: String,
    app: AppHandle,
    window: WebviewWindow,
) -> Result<(), ApiError> {
    check_hash(&hash)?;
    try_emit_to_owner(&app, &owner_label(&window), SHOW_IN_LOG_EVENT, hash)
        .map_err(|error| failed(error.to_string()))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Stdio;

    use super::*;
    use crate::test_support::TempRepo;

    const ANN: (&str, &str) = ("Ann Smith", "ann@example.com");
    const BOB: (&str, &str) = ("Bob Jones", "bob@example.com");
    const NFC_NAME: &str = "caf\u{e9}.txt";
    const NFD_NAME: &str = "cafe\u{301}.txt";

    // A commit of `file` with that content, by `who`, on that date (author and committer).
    fn commit_as(
        repo: &TempRepo,
        file: &str,
        content: &str,
        message: &str,
        who: (&str, &str),
        date: &str,
    ) -> String {
        let target = repo.path().join(file);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, content).unwrap();
        repo.git(&["add", "-A"]);
        let status = crate::test_support::git_command()
            .current_dir(repo.path())
            .args(["commit", "-q", "-m", message])
            .env("GIT_AUTHOR_NAME", who.0)
            .env("GIT_AUTHOR_EMAIL", who.1)
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .status()
            .unwrap();
        assert!(status.success());
        repo.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    fn real(repo: &TempRepo) -> impl FnMut(&[&str]) -> Result<String, GitError> + '_ {
        move |args| {
            git::run(repo.path(), args).map(|out| String::from_utf8_lossy(&out).into_owned())
        }
    }

    fn run(repo: &TempRepo, request: AnnotateRequest) -> Annotation {
        annotate(&mut real(repo), &request).unwrap()
    }

    fn at(path: &str, revision: &str) -> AnnotateRequest {
        AnnotateRequest {
            path: path.to_string(),
            revision: Some(revision.to_string()),
            ..AnnotateRequest::default()
        }
    }

    fn local(path: &str, text: &str) -> AnnotateRequest {
        AnnotateRequest {
            path: path.to_string(),
            local_text: Some(text.to_string()),
            ..AnnotateRequest::default()
        }
    }

    // The hash of each line's commit, `None` for a blank line.
    fn line_hashes(annotation: &Annotation) -> Vec<Option<String>> {
        annotation
            .lines
            .iter()
            .map(|line| line.map(|index| annotation.commits[index as usize].hash.clone()))
            .collect()
    }

    fn commit<'a>(annotation: &'a Annotation, hash: &str) -> &'a AnnotatedCommit {
        annotation
            .commits
            .iter()
            .find(|commit| commit.hash == hash)
            .unwrap()
    }

    fn three_commits(repo: &TempRepo) -> (String, String, String) {
        let first = commit_as(
            repo,
            "a.txt",
            "one\ntwo\nthree\n",
            "first\n\nits body",
            ANN,
            "2020-01-01T10:00:00Z",
        );
        let second = commit_as(
            repo,
            "a.txt",
            "one\nTWO\nthree\n",
            "second",
            BOB,
            "2020-01-02T10:00:00Z",
        );
        let third = commit_as(
            repo,
            "a.txt",
            "one\nTWO\nthree\nfour\n",
            "third",
            ANN,
            "2020-01-03T10:00:00Z",
        );
        (first, second, third)
    }

    #[test]
    fn blame_porcelain_maps_lines_to_revisions() {
        let repo = TempRepo::new();
        let (first, second, third) = three_commits(&repo);

        let annotation = run(&repo, at("a.txt", &third));

        assert_eq!(annotation.revision, third);
        assert_eq!(annotation.path, "a.txt");
        assert_eq!(
            line_hashes(&annotation),
            [&first, &second, &first, &third].map(|hash| Some(hash.clone()))
        );
        let oldest = commit(&annotation, &first);
        assert_eq!((oldest.author.as_str(), oldest.email.as_str()), ANN);
        assert_eq!(oldest.author_time, 1_577_872_800);
        assert_eq!(oldest.committer_time, 1_577_872_800);
        assert_eq!(oldest.subject, "first");
        assert_eq!(oldest.message, "first\n\nits body");
        assert_eq!(oldest.path, "a.txt");
        assert_eq!(oldest.previous, None);
        let previous = |hash: &str| {
            Some(FileRevision {
                hash: hash.to_string(),
                path: "a.txt".to_string(),
            })
        };
        assert_eq!(commit(&annotation, &second).previous, previous(&first));
        assert_eq!(commit(&annotation, &third).previous, previous(&second));
        // `HistoryIdColumn`: the oldest is 1.
        let numbers: Vec<_> = [&first, &second, &third]
            .iter()
            .map(|hash| commit(&annotation, hash).number)
            .collect();
        assert_eq!(numbers, [Some(1), Some(2), Some(3)]);
        assert_eq!(annotation.history_size, 3);
        assert_eq!(annotation.line, None);
    }

    #[test]
    fn one_revision_has_no_numbers_and_no_colours() {
        let repo = TempRepo::new();
        let only = commit_as(
            &repo,
            "a.txt",
            "one\ntwo\n",
            "only",
            ANN,
            "2020-01-01T10:00:00Z",
        );

        let annotation = run(&repo, at("a.txt", &only));

        assert_eq!(annotation.history_size, 0);
        let commit = &annotation.commits[0];
        assert_eq!(
            (commit.number, &commit.order_color, &commit.author_color),
            (None, &None, &None)
        );
    }

    #[test]
    fn locally_changed_lines_are_blank() {
        let repo = TempRepo::new();
        let (first, _, third) = three_commits(&repo);
        // A later commit of another file: the blame is of the last commit that touched this one.
        commit_as(
            &repo,
            "other.txt",
            "x\n",
            "other",
            BOB,
            "2020-01-04T10:00:00Z",
        );
        let text = "one\nTWO!\nthree\nfour\nfive\n";
        fs::write(repo.path().join("a.txt"), text).unwrap();

        let annotation = run(&repo, local("a.txt", text));

        assert_eq!(annotation.revision, third);
        assert_eq!(
            line_hashes(&annotation),
            [Some(first.clone()), None, Some(first), Some(third), None]
        );
    }

    #[test]
    fn a_text_without_its_last_newline_or_with_crlf_maps_every_line() {
        let repo = TempRepo::new();
        // The blob keeps its `\r\n` whatever the user's global `core.autocrlf` says, which
        // `TempRepo`'s own `git add` would read: the repository's setting wins.
        repo.git(&["config", "core.autocrlf", "false"]);
        let only = commit_as(
            &repo,
            "a.txt",
            "one\r\ntwo\r\n",
            "only",
            ANN,
            "2020-01-01T10:00:00Z",
        );

        let blob = repo.git(&["cat-file", "blob", "HEAD:a.txt"]);
        assert_eq!(blob, "one\r\ntwo\r\n", "the test must commit CRLF");

        let crlf = run(&repo, local("a.txt", "one\r\ntwo"));
        let lf = run(&repo, local("a.txt", "one\ntwo\n"));
        let at_revision = run(&repo, at("a.txt", &only));

        let both = [Some(only.clone()), Some(only.clone())];
        assert_eq!(line_hashes(&crlf), both);
        assert_eq!(line_hashes(&lf), both);
        assert_eq!(line_hashes(&at_revision), both);
        assert!(!crlf.suspicious);
        // An edit on disk keeps CRLF and maps again without a blame.
        let refreshed = refresh(
            &mut real(&repo),
            &RefreshRequest {
                path: "a.txt".to_string(),
                revision: only.clone(),
                blamed_path: "a.txt".to_string(),
                owners: crlf.owners.clone(),
                local_text: "one\r\nTWO\r\n".to_string(),
            },
        )
        .unwrap();
        assert_eq!(
            refreshed,
            Refreshed::Lines {
                lines: vec![Some(0), None],
                origins: vec![0, 1]
            }
        );
    }

    #[test]
    fn lines_ended_by_a_lone_cr_are_suspicious() {
        let repo = TempRepo::new();
        repo.git(&["config", "core.autocrlf", "false"]);
        // One line for blame, three for a document: `V:687`.
        let only = commit_as(
            &repo,
            "mac.txt",
            "a\rb\rc\n",
            "only",
            ANN,
            "2020-01-01T10:00:00Z",
        );

        assert!(run(&repo, at("mac.txt", &only)).suspicious);
        // Two lines for blame, three for a document after the last newline: one apart, which is not
        // (241's "different conventions about files ending with line separator").
        let unix = commit_as(
            &repo,
            "unix.txt",
            "a\nb\n",
            "unix",
            ANN,
            "2020-01-02T10:00:00Z",
        );
        assert!(!run(&repo, at("unix.txt", &unix)).suspicious);
    }

    #[test]
    fn colours_by_order_newest_first_with_the_dark_palette() {
        let repo = TempRepo::new();
        commit_as(&repo, "a.txt", "1\n", "one", ANN, "2020-01-01T10:00:00Z");
        commit_as(&repo, "a.txt", "1\n2\n", "two", BOB, "2020-01-02T10:00:00Z");
        // Two commits with the same committer date are one group.
        commit_as(
            &repo,
            "a.txt",
            "1\n2\n3\n",
            "three",
            BOB,
            "2020-01-03T10:00:00Z",
        );
        let last = commit_as(
            &repo,
            "a.txt",
            "1\n2\n3\n4\n",
            "four",
            ANN,
            "2020-01-03T10:00:00Z",
        );

        let annotation = run(&repo, at("a.txt", &last));

        let colours: Vec<_> = annotation
            .lines
            .iter()
            .map(|line| {
                let commit = &annotation.commits[line.unwrap() as usize];
                (
                    commit.order_color.as_deref(),
                    commit.author_color.as_deref(),
                )
            })
            .collect();
        // Three groups, newest first: `palette[21 * i / 3]`; the authors by name, `palette[k % 21]`
        // of the palette reordered by stride 4.
        assert_eq!(
            colours,
            [
                (Some("#242f45"), Some("#2f5194")),
                (Some("#2b3d61"), Some("#2d4473")),
                (Some("#2f5194"), Some("#2d4473")),
                (Some("#2f5194"), Some("#2f5194")),
            ]
        );
    }

    #[test]
    fn the_palettes_are_the_21_dark_colours() {
        assert_eq!(ORDER_COLORS.len(), 21);
        assert_eq!((ORDER_COLORS[0], ORDER_COLORS[20]), ("#2f5194", "#22252e"));
        assert_eq!(AUTHOR_COLORS.len(), 21);
        let mut sorted = AUTHOR_COLORS.to_vec();
        sorted.sort_unstable();
        let mut order = ORDER_COLORS.to_vec();
        order.sort_unstable();
        assert_eq!(sorted, order);
        assert_eq!(&AUTHOR_COLORS[..3], ["#2f5194", "#2d4473", "#2a3b5d"]);
    }

    #[test]
    fn annotate_previous_revision_opens_the_parent_at_the_translated_line() {
        let repo = TempRepo::new();
        let parent = commit_as(
            &repo,
            "a.txt",
            "a\nb\nc\nd\n",
            "parent",
            ANN,
            "2020-01-01T10:00:00Z",
        );
        let child_text = "z\na\nb\nC\nd\n";
        let child = commit_as(
            &repo,
            "a.txt",
            child_text,
            "child",
            BOB,
            "2020-01-02T10:00:00Z",
        );
        let annotation = run(&repo, at("a.txt", &child));
        let line = annotation.lines[3].unwrap() as usize;
        let previous = annotation.commits[line].previous.clone().unwrap();
        assert_eq!(
            previous,
            FileRevision {
                hash: parent.clone(),
                path: "a.txt".to_string()
            }
        );

        let opened = run(
            &repo,
            AnnotateRequest {
                from: Some(FromLine {
                    text: child_text.to_string(),
                    line: 4,
                }),
                ..at(&previous.path, &previous.hash)
            },
        );

        // `C` was changed from `c`: approximately the line where the change starts in the parent.
        assert_eq!(opened.revision, parent);
        assert_eq!(opened.line, Some(3));
        // A line after the changes moves by what they inserted.
        assert_eq!(translate_line(child_text, "a\nb\nc\nd\n", 5), 4);
        // Leading and trailing spaces do not count.
        assert_eq!(translate_line("  x\ny\n", "w\nx  \ny\n", 2), 3);
    }

    #[test]
    fn a_rename_gives_the_old_path_to_the_previous_revision() {
        let repo = TempRepo::new();
        let first = commit_as(
            &repo,
            "a.txt",
            "one\ntwo\nthree\nfour\n",
            "first",
            ANN,
            "2020-01-01T10:00:00Z",
        );
        repo.git(&["mv", "a.txt", "b.txt"]);
        let moved = commit_as(
            &repo,
            "b.txt",
            "one\ntwo\nthree\nfour\nfive\n",
            "moved",
            BOB,
            "2020-01-02T10:00:00Z",
        );

        let annotation = run(&repo, at("b.txt", &moved));

        assert_eq!(commit(&annotation, &first).path, "a.txt");
        assert_eq!(
            commit(&annotation, &moved).previous,
            Some(FileRevision {
                hash: first,
                path: "a.txt".to_string()
            })
        );
    }

    #[test]
    fn a_change_on_disk_recomputes_the_local_annotation() {
        let repo = TempRepo::new();
        let (_, _, third) = three_commits(&repo);
        let edited = "one\nTWO\nthree\nfour\nfive\n";
        fs::write(repo.path().join("a.txt"), edited).unwrap();
        let before = run(&repo, local("a.txt", edited));
        assert_eq!(line_hashes(&before)[4], None);

        // A commit from the terminal: the same text is now that commit's.
        let terminal = commit_as(
            &repo,
            "a.txt",
            edited,
            "from the terminal",
            BOB,
            "2020-01-05T10:00:00Z",
        );
        let after = run(&repo, local("a.txt", edited));

        assert_eq!(after.revision, terminal);
        assert_eq!(line_hashes(&after)[3], Some(third));
        assert_eq!(line_hashes(&after)[4], Some(terminal));
    }

    fn refresh_of(annotation: &Annotation, path: &str, text: &str) -> RefreshRequest {
        RefreshRequest {
            path: path.to_string(),
            revision: annotation.revision.clone(),
            blamed_path: annotation.path.clone(),
            owners: annotation.owners.clone(),
            local_text: text.to_string(),
        }
    }

    #[test]
    fn an_edit_on_disk_with_the_same_base_maps_the_lines_again() {
        let repo = TempRepo::new();
        let (first, _, third) = three_commits(&repo);
        let shown = "one\nTWO\nthree\nfour\n";
        let annotation = run(&repo, local("a.txt", shown));
        let edited = "zero\none\nTWO\nthree!\nfour\n";
        fs::write(repo.path().join("a.txt"), edited).unwrap();

        let refreshed =
            refresh(&mut real(&repo), &refresh_of(&annotation, "a.txt", edited)).unwrap();

        let Refreshed::Lines { lines, origins } = refreshed else {
            panic!("the base did not move: {refreshed:?}");
        };
        let hashes: Vec<_> = lines
            .iter()
            .map(|line| line.map(|index| annotation.commits[index as usize].hash.clone()))
            .collect();
        assert_eq!(hashes[0], None);
        assert_eq!(hashes[1], Some(first));
        assert_eq!(hashes[3], None);
        assert_eq!(hashes[4], Some(third));
        // `transferLineToVcs(line, approximate = true)`: a changed line goes where its change starts.
        assert_eq!(origins, [0, 0, 1, 2, 3]);
    }

    // `annotate_refresh` runs git without a task: with signed commits and `log.showSignature` on,
    // the signature check git prints must not make the base look moved.
    #[test]
    fn signatures_do_not_close_a_refreshed_annotation() {
        let repo = TempRepo::new();
        repo.sign_commits();
        let (_, _, third) = three_commits(&repo);
        assert!(repo
            .git(&["log", "-1", "--format=%H"])
            .contains("No signature"));
        let shown = "one\nTWO\nthree\nfour\n";
        let annotation = run(&repo, local("a.txt", shown));
        assert_eq!(annotation.revision, third);

        let refreshed =
            refresh(&mut real(&repo), &refresh_of(&annotation, "a.txt", shown)).unwrap();

        assert!(
            matches!(refreshed, Refreshed::Lines { .. }),
            "the base did not move: {refreshed:?}"
        );
    }

    #[test]
    fn a_commit_that_moves_the_base_closes_the_annotation() {
        let repo = TempRepo::new();
        three_commits(&repo);
        let edited = "one\nTWO\nthree\nfour\nfive\n";
        fs::write(repo.path().join("a.txt"), edited).unwrap();
        let annotation = run(&repo, local("a.txt", edited));
        // A commit of another file leaves the base where it was (`commit_as` would add `a.txt`).
        fs::write(repo.path().join("other.txt"), "x\n").unwrap();
        repo.git(&["add", "--", "other.txt"]);
        repo.git(&["commit", "-q", "-m", "other"]);
        let request = refresh_of(&annotation, "a.txt", edited);
        assert!(matches!(
            refresh(&mut real(&repo), &request).unwrap(),
            Refreshed::Lines { .. }
        ));

        commit_as(
            &repo,
            "a.txt",
            edited,
            "from the terminal",
            BOB,
            "2020-01-05T10:00:00Z",
        );
        assert_eq!(
            refresh(&mut real(&repo), &request).unwrap(),
            Refreshed::Closed
        );

        // A file removed by a commit has another last revision: closed too.
        let annotation = run(&repo, local("a.txt", edited));
        repo.git(&["rm", "-q", "a.txt"]);
        repo.git(&["commit", "-q", "-m", "removed"]);
        assert_eq!(
            refresh(&mut real(&repo), &refresh_of(&annotation, "a.txt", edited)).unwrap(),
            Refreshed::Closed
        );
    }

    #[test]
    fn blame_reaches_a_glob_named_and_nfd_file() {
        let repo = TempRepo::new();
        let glob = commit_as(
            &repo,
            "[a].txt",
            "glob\n",
            "glob",
            ANN,
            "2020-01-01T10:00:00Z",
        );
        // `a.txt` matches `[a].txt` as a pattern: its later commit is not the file's.
        commit_as(
            &repo,
            "a.txt",
            "plain\n",
            "plain",
            BOB,
            "2020-01-02T10:00:00Z",
        );
        let dash = commit_as(
            &repo,
            "-n.txt",
            "dash\n",
            "dash",
            BOB,
            "2020-01-03T10:00:00Z",
        );
        stage_exact(&repo, NFC_NAME, "composed\n");
        stage_exact(&repo, NFD_NAME, "decomposed\n");
        repo.git(&["commit", "-q", "-m", "both forms"]);
        let both = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
        repo.git(&["config", "core.precomposeunicode", "true"]);

        assert_eq!(run(&repo, local("[a].txt", "glob\n")).revision, glob);
        assert_eq!(run(&repo, local("-n.txt", "dash\n")).revision, dash);
        let decomposed = run(&repo, at(NFD_NAME, &both));
        assert_eq!(
            (decomposed.path.as_str(), decomposed.lines.len()),
            (NFD_NAME, 1)
        );
        assert_eq!(decomposed.commits[0].path, NFD_NAME);
    }

    #[test]
    fn a_revision_that_is_not_a_full_hash_never_reaches_git() {
        let repo = TempRepo::new();
        three_commits(&repo);
        let mut asked = Vec::new();
        let mut git = |args: &[&str]| {
            asked.push(args.join(" "));
            Ok(String::new())
        };

        let refused = annotate(&mut git, &at("a.txt", "--output=/tmp/x"));

        assert!(refused.is_err());
        assert!(asked.is_empty(), "{asked:?}");
    }

    #[test]
    fn quoted_names_are_read_back() {
        assert_eq!(unquoted("plain name.txt"), "plain name.txt");
        assert_eq!(unquoted(r#""a\"b\\c\t.txt""#), "a\"b\\c\t.txt");
        assert_eq!(unquoted(r#""cafe\314\201.txt""#), NFD_NAME);
    }

    // Review Focus 4: time and memory of a large annotation, run on demand
    // (`cargo test --lib annotate::tests::large -- --ignored --nocapture`, under `/usr/bin/time -l`).
    // `RAMIFY_ANNOTATE_MB` picks the size; a local text with edits every 1000 lines is mapped too.
    #[test]
    #[ignore]
    fn large_file_annotation_time() {
        let megabytes: usize = std::env::var("RAMIFY_ANNOTATE_MB")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(30);
        let repo = TempRepo::new();
        let line = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcde\n";
        let count = megabytes * 1024 * 1024 / line.len();
        let first = line.repeat(count);
        commit_as(
            &repo,
            "big.txt",
            &first,
            "first",
            ANN,
            "2020-01-01T10:00:00Z",
        );
        let second: String = (0..count)
            .map(|index| {
                if index % 100 == 0 {
                    "changed line\n"
                } else {
                    line
                }
            })
            .collect();
        let head = commit_as(
            &repo,
            "big.txt",
            &second,
            "second",
            BOB,
            "2020-01-02T10:00:00Z",
        );
        let edited: String = (0..count)
            .map(|index| {
                if index % 1000 == 7 {
                    "edited\n"
                } else if index % 100 == 0 {
                    "changed line\n"
                } else {
                    line
                }
            })
            .collect();

        let started = std::time::Instant::now();
        let at_revision = run(&repo, at("big.txt", &head));
        let revision_time = started.elapsed();
        let started = std::time::Instant::now();
        let annotated = run(&repo, local("big.txt", &edited));
        let local_time = started.elapsed();

        eprintln!(
            "{megabytes} MB, {count} lines: revision {revision_time:?}, local {local_time:?}, {} blank",
            annotated.lines.iter().filter(|line| line.is_none()).count()
        );
        assert_eq!(at_revision.lines.len(), count);
        assert_eq!(annotated.lines.len(), count);
    }

    // An index entry with exactly that name: the file system could not hold both forms.
    fn stage_exact(repo: &TempRepo, name: &str, content: &str) {
        let blob = crate::test_support::git_command()
            .current_dir(repo.path())
            .args(["hash-object", "-w", "--stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
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
}
