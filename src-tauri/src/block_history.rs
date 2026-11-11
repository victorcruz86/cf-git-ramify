// Show History for Selection (`Vcs.ShowHistoryForBlock`): the `BlockLoader` of 241's
// `VcsSelectionHistoryDialog`, with `Block` (`platform/vcs-impl/src/com/intellij/diff/Block.java`)
// and `DiffUtil.updateRangeOnModification` ported, and the window that shows it (decision 7 of
// phase 4 part 4: it starts from the viewer).

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use similar::{capture_diff_slices_deadline, Algorithm, DiffOp};
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::commands::{
    blocking_api, current_repo, emit_to_owner, failed, owner_label, reveal, session_of,
    try_emit_to_owner, ApiError, AppState, WindowSession,
};
use crate::diff::check_hash;
use crate::git::{self, GitError};
use crate::history::{self, Git, Starts, Walk};
use crate::task::{Runner, TaskId, TaskKind};
use crate::windows::{self, WindowKind};
use crate::worktree_diff;

const WINDOW_URL: &str = "index.html?window=selection-history";
// The page takes what is waiting with `selection_history_target`: on mounting, and on each of these.
const OPEN_EVENT: &str = "selection-history-open";
// `Show All Affected Files`, to the main window, whose `Changes` window shows it.
const SHOW_AFFECTED_EVENT: &str = "selection-history-show-affected";
// What the stream has found so far, to the window only.
const UPDATE_EVENT: &str = "selection-history-update";
// `show.history.dialog.title.template` with `action.name.show.history.for.selection` (`V:626-627`).
// 241 runs the loader without a progress; Ramify's task needs a title, and takes the window's.
const TITLE: &str = "History for Selection";
// Deadline of each line diff: once past, Myers ends with a valid alignment, though not minimal, so
// a huge revision does not hold the stream.
const LINE_DIFF_DEADLINE: Duration = Duration::from_millis(500);

//
// Block
//

// `Block`: the lines of a text and a range of them, half-open and from 0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    source: Vec<String>,
    start: usize,
    end: usize,
}

impl Block {
    // `Block(String[], int, int)`: the range is bound to the lines (`DiffUtil.bound`).
    pub fn new(source: Vec<String>, start: i64, end: i64) -> Self {
        let length = source.len() as i64;
        let start = start.clamp(0, length);
        let end = end.clamp(start, length);
        Self {
            source,
            start: start as usize,
            end: end as usize,
        }
    }

    // `Block(String, int, int)`.
    pub fn of_text(text: &str, start: i64, end: i64) -> Self {
        Self::new(tokenize(text), start, end)
    }

    pub fn start(&self) -> usize {
        self.start
    }

    pub fn end(&self) -> usize {
        self.end
    }

    // `getLines`.
    pub fn lines(&self) -> &[String] {
        &self.source[self.start..self.end]
    }

    // `getBlockContent`: the lines joined by `\n`.
    pub fn content(&self) -> String {
        self.lines().join("\n")
    }

    // `createPreviousBlock`: where the range was in the previous text, by the line diff of the
    // two ignoring whitespace (`ByLine.compare(prev, cur, IGNORE_WHITESPACES)`).
    pub fn create_previous_block(&self, previous: Vec<String>) -> Block {
        let Some(changes) = line_changes(&previous, &self.source, LINE_DIFF_DEADLINE) else {
            return Block::new(previous, 0, 0);
        };
        // "Empty range should not be transferred to the non-empty range."
        let greedy = self.start != self.end;
        let (mut start, mut end) = (self.start as i64, self.end as i64);
        let mut shift = 0;
        for change in changes {
            let change_start = change.start2 as i64 + shift;
            let change_end = change.end2 as i64 + shift;
            let change_shift =
                (change.end1 - change.start1) as i64 - (change.end2 - change.start2) as i64;
            (start, end) = update_range_on_modification(
                start,
                end,
                change_start,
                change_end,
                change_shift,
                greedy,
            );
            shift += change_shift;
        }
        // 241 only logs a range outside the text; `Block` bounds it.
        Block::new(previous, start, end)
    }
}

// `Block.tokenize` = `LineTokenizer.tokenize(text, false, false)`: `\r\n`, `\r` and `\n` end a
// line, without the separator, and a text that ends with one has a last empty line.
pub fn tokenize(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let (mut start, mut at) = (0, 0);
    while at < bytes.len() {
        match bytes[at] {
            b'\r' | b'\n' => {
                lines.push(text[start..at].to_string());
                at += if bytes[at] == b'\r' && bytes.get(at + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
                start = at;
            }
            _ => at += 1,
        }
    }
    lines.push(text[start..].to_string());
    lines
}

// `DiffUtil.updateRangeOnModification` (241:1336-1370): where `[start, end)` goes when the lines
// `[change_start, change_end)` are replaced by `change_end - change_start + shift` lines.
pub fn update_range_on_modification(
    start: i64,
    end: i64,
    change_start: i64,
    change_end: i64,
    shift: i64,
    greedy: bool,
) -> (i64, i64) {
    if end <= change_start {
        // The change is after the range.
        return (start, end);
    }
    if start >= change_end {
        // Before it.
        return (start + shift, end + shift);
    }
    if start <= change_start && end >= change_end {
        // Inside it.
        return (start, end + shift);
    }
    // The range is damaged: an approximate new position.
    let new_change_end = change_end + shift;
    if start >= change_start && end <= change_end {
        // Fully inside the change.
        return if greedy {
            (change_start, new_change_end)
        } else {
            (new_change_end, new_change_end)
        };
    }
    if start < change_start {
        // Its bottom boundary.
        if greedy {
            (start, new_change_end)
        } else {
            (start, change_start)
        }
    } else if greedy {
        // Its top boundary.
        (change_start, end + shift)
    } else {
        (new_change_end, end + shift)
    }
}

// `Range`: lines `[start1, end1)` of the first text replaced by `[start2, end2)` of the second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Change {
    start1: usize,
    end1: usize,
    start2: usize,
    end2: usize,
}

// `DiffConfig.DELTA_THRESHOLD_SIZE`, the floor of `MyersLCS.executeWithThreshold`'s limit.
const DELTA_THRESHOLD_SIZE: usize = 20000;

// `MyersLCS.executeWithThreshold` fails when the differences exceed
// `max(20000 + 10·√(n + m), DELTA_THRESHOLD_SIZE)`, counted here over the whole texts (241 first
// leaves out the lines found on one side only, and retries with patience before giving up).
fn too_big(len1: usize, len2: usize, matched: usize) -> bool {
    let threshold = DELTA_THRESHOLD_SIZE.max(20000 + 10 * ((len1 + len2) as f64).sqrt() as usize);
    len1 + len2 - 2 * matched > threshold
}

// `TrimUtil.isWhiteSpace`, as merge3.rs: what `IGNORE_WHITESPACES` leaves out of a line.
fn without_whitespace(line: &str) -> String {
    line.chars()
        .filter(|c| !matches!(c, ' ' | '\t' | '\n'))
        .collect()
}

// `ByLine.compare(lines1, lines2, IGNORE_WHITESPACES).iterateChanges()`. As in merge3.rs, the diff
// is `similar`'s Myers over the lines without whitespace, not IntelliJ's "fair" diff: where two
// alignments tie the ranges may differ. Past the deadline Myers still ends with a valid alignment,
// though not a minimal one, and that is kept: a slow machine moves a block less precisely instead
// of losing it. `None` only past `MyersLCS`'s limit (`DiffTooBigException`, by size, not time).
fn line_changes(lines1: &[String], lines2: &[String], limit: Duration) -> Option<Vec<Change>> {
    let keys1: Vec<String> = lines1.iter().map(|line| without_whitespace(line)).collect();
    let keys2: Vec<String> = lines2.iter().map(|line| without_whitespace(line)).collect();
    let deadline = Instant::now() + limit;
    let ops = capture_diff_slices_deadline(Algorithm::Myers, &keys1, &keys2, Some(deadline));
    let mut changes = Vec::new();
    let (mut last1, mut last2) = (0, 0);
    let equal = ops.iter().filter_map(|op| match *op {
        DiffOp::Equal {
            old_index,
            new_index,
            len,
        } => Some((old_index, new_index, len)),
        _ => None,
    });
    let matched: usize = equal.clone().map(|(_, _, len)| len).sum();
    if too_big(keys1.len(), keys2.len(), matched) {
        return None;
    }
    for (start1, start2, len) in equal.chain([(keys1.len(), keys2.len(), 0)]) {
        if start1 > last1 || start2 > last2 {
            changes.push(Change {
                start1: last1,
                end1: start1,
                start2: last2,
                end2: start2,
            });
        }
        (last1, last2) = (start1 + len, start2 + len);
    }
    Some(changes)
}

//
// The stream
//

// What the viewer asks for: the local file, the lines selected (from 0, both included, as
// `VcsSelection.getSelectionStartLineNumber` and `getSelectionEndLineNumber`) and its text as the
// viewer shows it (`document.getText()`). `stream` tells the page's answers apart.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SelectionRequest {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub local_text: String,
    pub stream: u32,
}

// A `GitFileRevision` of the history, with what the table and `Commit Message:` show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionRevision {
    pub hash: String,
    // The file's path in this revision (`GitFileRevision.getPath`).
    pub path: String,
    // `getRevisionDate` = `GitRevisionNumber.getTimestamp`, the commit time, in seconds.
    pub commit_time: i64,
    pub author: String,
    pub author_email: String,
    pub committer: String,
    pub committer_email: String,
    // `GitLogRecord.getFullMessage`.
    pub message: String,
}

// The lines of a block and where they start in their text (`LINE_NUMBER_CONVERTOR`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockLines {
    pub start: usize,
    pub lines: Vec<String>,
}

impl From<&Block> for BlockLines {
    fn from(block: &Block) -> Self {
        Self {
            start: block.start,
            lines: block.lines().to_vec(),
        }
    }
}

// What the stream reports, in order: the block of the local text (index 0), the revisions of the
// history, then one block per revision, newest first (index `i` is revision `i - 1`). `None` is
// `EMPTY_BLOCK`: the range is gone there, and so in every older revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SelectionUpdate {
    #[serde(rename_all = "camelCase")]
    Revisions {
        stream: u32,
        // `CurrentRevision.getRevisionDate`: the local file's modification time, in seconds.
        local_time: Option<i64>,
        revisions: Vec<SelectionRevision>,
    },
    #[serde(rename_all = "camelCase")]
    Block {
        stream: u32,
        index: usize,
        block: Option<BlockLines>,
    },
}

// `BlockLoader.start`: the history 241 collects for it (`VcsCachingHistory.collectSession` →
// `GitHistoryProvider.reportAppendableHistory`: `GitFileHistory` from HEAD without `fullHistory`,
// so `Walk::FirstAddition`, at most `MAX_COUNT` per walk), then each revision's text through
// `git::exact` at its own path and its block from the newer one. HEAD is read once, first: a
// commit made while it runs leaves the stream on the history it started with, which every blob it
// reads belongs to. `cancel` is asked between revisions; a failure ends it, as `VcsException`.
pub fn selection_history(
    git: &mut Git,
    request: &SelectionRequest,
    local_time: Option<i64>,
    emit: &mut dyn FnMut(SelectionUpdate),
    cancel: &dyn Fn() -> Result<(), GitError>,
) -> Result<(), GitError> {
    let stream = request.stream;
    // `BlockLoader`'s constructor: the selected lines of the document, the end line included.
    let local = Block::of_text(
        &request.local_text,
        i64::from(request.start_line),
        i64::from(request.end_line) + 1,
    );
    emit(SelectionUpdate::Block {
        stream,
        index: 0,
        block: Some(BlockLines::from(&local)),
    });
    let revisions = match head_commit(git)? {
        Some(head) => {
            let path = history::last_commit_path(git, &request.path);
            let rows = history::file_history(
                git,
                &path,
                &Starts::Revisions(vec![head]),
                Walk::FirstAddition,
            )?;
            with_details(git, &rows)?
        }
        None => Vec::new(),
    };
    emit(SelectionUpdate::Revisions {
        stream,
        local_time,
        revisions: revisions.clone(),
    });
    // `createBlock`: once the range is gone it stays gone, and that revision is not read.
    let mut block = Some(local);
    for (index, revision) in revisions.iter().enumerate() {
        cancel()?;
        block = match block {
            Some(newer) => {
                let text = blob_text(git, &revision.hash, &revision.path)?;
                let older = newer.create_previous_block(tokenize(&text));
                (older.start != older.end).then_some(older)
            }
            None => None,
        };
        emit(SelectionUpdate::Block {
            stream,
            index: index + 1,
            block: block.as_ref().map(BlockLines::from),
        });
    }
    Ok(())
}

// The commit HEAD names now; `None` without commits, where 241's history is empty.
fn head_commit(git: &mut Git) -> Result<Option<String>, GitError> {
    match git(&["rev-parse", "-q", "--verify", "HEAD^{commit}"]) {
        Ok(hash) => Ok(Some(hash.trim().to_string())),
        Err(GitError::Failed { code: Some(1), .. }) => Ok(None),
        Err(error) => Err(error),
    }
}

const DETAILS_FORMAT: &str = "--format=%x01%H%x00%ct%x00%an%x00%ae%x00%cn%x00%ce%x00%B";
const DETAILS_FIELDS: usize = 7;

// `GitFileHistory.createLogParser`: what each revision of the history shows, in its order.
fn with_details(
    git: &mut Git,
    rows: &[history::HistoryRow],
) -> Result<Vec<SelectionRevision>, GitError> {
    let mut found: HashMap<String, Vec<String>> = HashMap::new();
    for chunk in rows.chunks(git::PATHS_PER_COMMAND) {
        let mut args = vec![
            history::NO_SIGNATURES[0],
            history::NO_SIGNATURES[1],
            "log",
            "--no-walk=unsorted",
            DETAILS_FORMAT,
            "--encoding=UTF-8",
            "--end-of-options",
        ];
        args.extend(chunk.iter().map(|row| row.hash.as_str()));
        for record in git(&args)?.split('\u{1}') {
            let fields: Vec<String> = record
                .splitn(DETAILS_FIELDS, '\0')
                .map(str::to_string)
                .collect();
            if fields.len() == DETAILS_FIELDS {
                found.insert(fields[0].clone(), fields);
            }
        }
    }
    rows.iter()
        .map(|row| {
            let fields = found.get(row.hash.as_str()).ok_or_else(|| {
                GitError::Io(format!("Cannot read the details of commit {}", row.hash))
            })?;
            Ok(SelectionRevision {
                hash: row.hash.clone(),
                path: row.path.clone(),
                commit_time: fields[1]
                    .parse()
                    .map_err(|_| GitError::Io(format!("unreadable commit time: {}", fields[1])))?,
                author: fields[2].to_string(),
                author_email: fields[3].to_string(),
                committer: fields[4].to_string(),
                committer_email: fields[5].to_string(),
                message: fields[6].trim().to_string(),
            })
        })
        .collect()
}

// `GitFileRevision.loadContent`: the blob at the revision's own path, with the exact bytes of the
// name, without the byte order mark the viewer does not show of the local text either.
fn blob_text(git: &mut Git, revision: &str, path: &str) -> Result<String, GitError> {
    let object = format!("{revision}:{path}");
    let text = git(&git::exact(&["cat-file", "blob", &object]))?;
    Ok(match text.strip_prefix('\u{feff}') {
        Some(rest) => rest.to_string(),
        None => text,
    })
}

//
// The window
//

// What `selection_history_open` left for the page, and the stream it runs, live in the project
// window's session.
fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// One window, reused: a new selection replaces what it shows, as Ramify has one window per kind
// (241 opens a frame each time). Like the viewer, it is created hidden and its page shows it once
// mounted; the selection waits here until the page takes it.
#[tauri::command]
pub async fn selection_history_open(
    request: SelectionRequest,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    let repo = current_repo(&state, &window)?;
    worktree_diff::check_inside(&repo, &request.path)?;
    let session = session_of(&state, window.label())?;
    *locked(&session.selection_pending) = Some(request);
    let owner = owner_label(&window);
    let label = windows::label_for(WindowKind::SelectionHistory, &owner);
    let opened = match app.get_webview_window(&label) {
        Some(history) => {
            reveal(&history).and_then(|()| try_emit_to_owner(&app, &owner, OPEN_EVENT, ()))
        }
        None => WebviewWindowBuilder::new(&app, &label, WebviewUrl::App(WINDOW_URL.into()))
            .title(TITLE)
            .inner_size(900.0, 700.0)
            .visible(false)
            .background_color(tauri::webview::Color(0x1e, 0x1f, 0x22, 0xff))
            .build()
            .map(drop),
    };
    opened.map_err(|error| failed(error.to_string()))
}

#[tauri::command]
pub fn selection_history_target(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Option<SelectionRequest>, ApiError> {
    let session = session_of(&state, window.label())?;
    let request = locked(&session.selection_pending).take();
    Ok(request)
}

// The stream of a selection: a task that a newer one, closing the window or the status bar
// cancels. Its updates go to the window; the task ends with nothing, or with the failure.
#[tauri::command]
pub async fn selection_history_start(
    request: SelectionRequest,
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    let repo = current_repo(&state, &window)?;
    worktree_diff::check_inside(&repo, &request.path)?;
    let path = repo.join(&request.path);
    let local_time = blocking_api(move || Ok(modified_seconds(&path))).await?;
    let owner = owner_label(&window);
    let session = session_of(&state, window.label())?;
    Ok(start_stream(
        &state.tasks,
        &session.selection_running,
        &owner.clone(),
        &repo,
        request,
        local_time,
        move |update| {
            emit_to_owner(&app, &owner, UPDATE_EVENT, update);
        },
    ))
}

// The stream's task, in place of the one before it, which is cancelled.
fn start_stream(
    tasks: &Runner,
    slot: &Mutex<Option<(String, TaskId)>>,
    owner: &str,
    repo: &Path,
    request: SelectionRequest,
    local_time: Option<i64>,
    emit: impl Fn(SelectionUpdate) + Send + 'static,
) -> TaskId {
    let mut running = locked(slot);
    if let Some((previous_owner, previous)) = running.take() {
        tasks.cancel(&previous_owner, previous);
    }
    let id = tasks.start(
        owner,
        repo,
        TaskKind::Read,
        TITLE.to_string(),
        move |context| {
            let mut git = |args: &[&str]| context.git(args).map(|output| output.stdout);
            selection_history(
                &mut git,
                &request,
                local_time,
                // A cancelled stream says nothing more: a reopened window starts its streams anew.
                &mut |update| {
                    if context.check_cancelled().is_ok() {
                        emit(update);
                    }
                },
                &|| context.check_cancelled(),
            )
        },
    );
    *running = Some((owner.to_string(), id));
    id
}

// What closing the window leaves behind: no selection waiting, no stream running.
pub fn release(tasks: &Runner, session: &WindowSession) {
    locked(&session.selection_pending).take();
    if let Some((owner, id)) = locked(&session.selection_running).take() {
        tasks.cancel(&owner, id);
    }
}

// `CurrentRevision.getRevisionDate` = `VirtualFile.getTimeStamp`.
fn modified_seconds(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let seconds = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    i64::try_from(seconds).ok()
}

// `Show All Affected Files` of a row: the main window shows the commit in its `Changes` window,
// with the file as it was in that revision chosen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AffectedFiles {
    pub hash: String,
    pub path: String,
}

#[tauri::command]
pub fn selection_history_show_affected(
    affected: AffectedFiles,
    app: AppHandle,
    window: WebviewWindow,
) -> Result<(), ApiError> {
    check_hash(&affected.hash)?;
    try_emit_to_owner(&app, &owner_label(&window), SHOW_AFFECTED_EVENT, affected)
        .map_err(|error| failed(error.to_string()))
}

// `closeOnEsc` from the page.
#[tauri::command]
pub fn selection_history_close(app: AppHandle, window: WebviewWindow) {
    close(&app, &owner_label(&window));
}

// Closing it, or opening another repository: its stream is cancelled and the window goes.
pub fn close(app: &AppHandle, owner: &str) {
    let state = app.state::<AppState>();
    if let Ok(session) = session_of(&state, owner) {
        release(&state.tasks, &session);
    }
    if let Some(window) =
        app.get_webview_window(&windows::label_for(WindowKind::SelectionHistory, owner))
    {
        let _ = window.destroy();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fs;
    use std::sync::{mpsc, Arc};

    use super::*;
    use crate::test_support::TempRepo;

    type Lines = &'static [&'static str];

    // `SelectedBlockHistoryTest` (241, `platform/platform-tests/testSrc/com/intellij/diff/
    // SelectedBlockHistoryTest.java`, Apache 2.0), lines 17-384 as NOTICE says: here each
    // `doTest(beforePrev, prevBlock, afterPrev, before, block, after)` call of 17-349, in order,
    // with its test's name; `testContent` (351-359) is at the end of the test below, and `doTest`
    // and `composeVersion` (361-384) are `previous_block` and `compose`.
    #[rustfmt::skip]
    const CASES: &[(&str, [Lines; 6])] = &[
        ("test1", [&["1", "2", "3"], &["4", "5", "6"], &["7", "8", "9"], &["1", "2", "3"], &["4", "52", "6"], &["7", "8", "9"]]),
        ("test2", [&["1", "1", "2", "3"], &["4", "5", "6"], &["7", "8", "9"], &["1", "2", "3"], &["4", "52", "6"], &["7", "8", "9"]]),
        ("test3", [&["1", "2", "3"], &["4", "5", "6"], &["7", "8", "9"], &["1", "2", "3"], &["4", "5", "6"], &["7", "8", "9"]]),
        ("test4", [&[], &["4", "5", "6"], &["7", "8", "9"], &["1", "2", "3"], &["4", "5", "6"], &["7", "8", "9"]]),
        ("test5", [&["1", "2", "3"], &["4", "5", "6"], &[], &["1", "2", "3"], &["4", "5", "6"], &["7", "8", "9"]]),
        ("test6", [&["1", "2", "3"], &["4", "5", "6"], &["7", "8", "9"], &[], &["4", "5", "6"], &["7", "8", "9"]]),
        ("test7", [&["1", "2", "3"], &["4", "5", "6"], &["7", "8", "9"], &["1", "2", "3"], &["4", "5", "6"], &[]]),
        ("test8", [&["1", "2", "3"], &["4", "5", "6"], &["7", "8", "9"], &["1", "2", "3"], &["44", "4", "5", "6", "66"], &["7", "8", "9"]]),
        ("test9", [&["1", "2", "3"], &["4", "5", "6"], &["7", "8", "9"], &["1", "2", "3"], &["44", "4", "55", "6", "66"], &["7", "8", "9"]]),
        ("test10", [&["0.5", "1", "3", "3.5"], &["4", "5", "5.5", "6"], &["6,5", "7", "8", "9", "9.5"], &["+0.5", "1", "2", "3", "3.5+", "3.5++"], &["4", "5", "5.5+", "6"], &["+6,5", "++6,5", "7", "9", "9.5+", "9.5++"]]),
        ("test11", [&["1", "2", "3"], &["a", "a"], &["7", "8", "9"], &["1", "2", "3"], &["b"], &["7", "8", "9"]]),
        ("test12", [&["1", "2", "3"], &[], &["7", "8", "9"], &["1", "2", "3"], &["4", "5", "6"], &["7", "8", "9"]]),
        ("test13", [&["1", "2", "3"], &["4", "5", "6"], &["7", "8", "9"], &[], &["4", "52", "6"], &[]]),
        ("test14", [&[], &["4", "52", "6"], &[], &["1", "2", "3"], &["4", "5", "6"], &["7", "8", "9"]]),
        ("testGreediness", [&[], &["x", "y", "z"], &[], &["1"], &["2", "3"], &["4"]]),
        ("testGreediness", [&["1"], &["0"], &["3"], &["1", "4"], &["5", "0"], &["3"]]),
        ("testGreediness", [&["1", "2"], &["3", "5", "X", "Z", "7"], &["8", "9"], &["1", "2"], &["3", "4", "5", "6", "7"], &["8", "9"]]),
        ("testGreediness", [&["1"], &["5", "X", "Z"], &["9"], &["1", "2"], &["3", "4", "5", "6", "7"], &["8", "9"]]),
        ("testGreediness", [&["1"], &["5", "X", "7"], &["Z", "9"], &["1", "2"], &["3", "4", "5", "6", "7"], &["8", "9"]]),
        ("testEmptyPreviousText", [&[], &[], &[], &[], &[], &[]]),
        ("testEmptyPreviousText", [&[], &[], &[], &[], &[], &["x"]]),
        ("testEmptyPreviousText", [&[], &[], &[], &["x"], &[], &[]]),
        ("testEmptyPreviousText", [&[], &[], &[], &[], &["x"], &[]]),
        ("testEmptyPreviousText", [&[], &[], &[], &["z"], &["x"], &["y"]]),
        ("testEmptyCurrentRange", [&[], &[], &["x"], &[], &[], &[]]),
        ("testEmptyCurrentRange", [&[], &[], &["x", "y", "z"], &[], &[], &[]]),
        ("testEmptyCurrentRange", [&[], &[], &["x"], &[], &[], &["z"]]),
        ("testEmptyCurrentRange", [&[], &[], &["x"], &[], &[], &["y", "z"]]),
        ("testEmptyCurrentRange", [&["x"], &[], &[], &["y"], &[], &["z"]]),
        ("testEmptyCurrentRange", [&["y"], &[], &["w", "z"], &["y"], &[], &["z"]]),
    ];

    // `composeVersion`.
    fn compose(parts: &[Lines]) -> Vec<String> {
        parts
            .iter()
            .flat_map(|part| part.iter().map(|line| line.to_string()))
            .collect()
    }

    // `doTest`: the block of the current version, moved to the previous one, is the expected
    // block of the previous version.
    fn previous_block(case: &[Lines; 6]) -> (Block, Block) {
        let [before_prev, prev_block, after_prev, before, block, after] = *case;
        let prev_version = compose(&[before_prev, prev_block, after_prev]);
        let current_version = compose(&[before, block, after]);
        let start = before.len() as i64;
        let actual = Block::new(current_version, start, start + block.len() as i64)
            .create_previous_block(prev_version.clone());
        let start = before_prev.len() as i64;
        let expected = Block::new(prev_version, start, start + prev_block.len() as i64);
        (expected, actual)
    }

    #[test]
    fn selected_block_history_cases_from_intellij() {
        for (index, (name, case)) in CASES.iter().enumerate() {
            let (expected, actual) = previous_block(case);
            assert_eq!(actual, expected, "{name}, call {index}");
        }
        // `testContent`.
        assert_eq!(
            Block::of_text("0\n1\n2\n3\n4\n5\n6", 3, 7).content(),
            "3\n4\n5\n6"
        );
        assert_eq!(
            Block::of_text("0\n1\n2\n3\n4\n5\n6\n", 3, 7).content(),
            "3\n4\n5\n6"
        );
    }

    #[test]
    fn lines_are_split_as_line_tokenizer_does() {
        assert_eq!(tokenize(""), Vec::<String>::new());
        assert_eq!(tokenize("a"), ["a"]);
        assert_eq!(tokenize("a\n"), ["a", ""]);
        assert_eq!(tokenize("\n"), ["", ""]);
        assert_eq!(tokenize("a\r\nb\rc\nd"), ["a", "b", "c", "d"]);
    }

    #[test]
    fn a_line_changed_only_in_whitespace_keeps_the_block() {
        let current = Block::of_text("a\n  b  \nc\n", 1, 2);
        let previous = current.create_previous_block(tokenize("a\nb\nc\n"));
        assert_eq!((previous.start(), previous.end()), (1, 2));
        assert_eq!(previous.content(), "b");
    }

    // The deadline bounds the time, not the result: past it the alignment is kept, and only the
    // size limit of `MyersLCS` loses the block, whatever the machine's load.
    #[test]
    fn a_diff_past_its_deadline_keeps_the_block_and_only_size_loses_it() {
        let numbered = |prefix: &str, count: usize| -> Vec<String> {
            (0..count).map(|line| format!("{prefix}{line}")).collect()
        };
        let mut previous = numbered("line ", 3000);
        previous.insert(1500, "added".to_string());
        let changes = line_changes(&previous, &numbered("line ", 3000), Duration::ZERO).unwrap();
        let removed: usize = changes
            .iter()
            .map(|change| change.end1 - change.start1)
            .sum();
        let added: usize = changes
            .iter()
            .map(|change| change.end2 - change.start2)
            .sum();
        assert_eq!(removed - added, 1);

        let (old, new) = (numbered("old ", 15000), numbered("new ", 15000));
        assert_eq!(line_changes(&old, &new, Duration::ZERO), None);
        let block = Block::new(new, 10, 20).create_previous_block(old);
        assert_eq!((block.start(), block.end()), (0, 0));
    }

    fn real(repo: &Path) -> impl FnMut(&[&str]) -> Result<String, GitError> + '_ {
        move |args| git::run(repo, args).map(|out| String::from_utf8_lossy(&out).into_owned())
    }

    fn head(repo: &TempRepo) -> String {
        repo.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    fn request(path: &str, start: u32, end: u32, text: &str) -> SelectionRequest {
        SelectionRequest {
            path: path.to_string(),
            start_line: start,
            end_line: end,
            local_text: text.to_string(),
            stream: 7,
        }
    }

    // The updates of a whole stream.
    fn streamed(repo: &TempRepo, request: &SelectionRequest) -> Vec<SelectionUpdate> {
        let mut updates = Vec::new();
        selection_history(
            &mut real(repo.path()),
            request,
            Some(5),
            &mut |update| updates.push(update),
            &|| Ok(()),
        )
        .unwrap();
        updates
    }

    // The blocks of a stream, by index, as their lines (`None` is `EMPTY_BLOCK`).
    fn blocks(updates: &[SelectionUpdate]) -> Vec<Option<Vec<String>>> {
        updates
            .iter()
            .filter_map(|update| match update {
                SelectionUpdate::Block { block, .. } => {
                    Some(block.as_ref().map(|b| b.lines.clone()))
                }
                SelectionUpdate::Revisions { .. } => None,
            })
            .collect()
    }

    fn revisions(updates: &[SelectionUpdate]) -> Vec<SelectionRevision> {
        updates
            .iter()
            .find_map(|update| match update {
                SelectionUpdate::Revisions { revisions, .. } => Some(revisions.clone()),
                SelectionUpdate::Block { .. } => None,
            })
            .unwrap()
    }

    fn lines(text: &[&str]) -> Option<Vec<String>> {
        Some(text.iter().map(|line| line.to_string()).collect())
    }

    #[test]
    fn the_stream_gives_one_block_per_revision_newest_first() {
        let repo = TempRepo::new();
        let first = repo.commit("a.txt", b"a\nb\nc\nd\n", "first");
        let second = repo.commit("a.txt", b"a\nb2\nc\nd\n", "second\n\nbody");
        let third = repo.commit("a.txt", b"a\nb2\nc\nd2\n", "third");
        let updates = streamed(&repo, &request("a.txt", 1, 2, "x\na\nb3\nc\nd2\n"));

        let found = revisions(&updates);
        let hashes: Vec<&str> = found
            .iter()
            .map(|revision| revision.hash.as_str())
            .collect();
        assert_eq!(hashes, [third.as_str(), second.as_str(), first.as_str()]);
        assert_eq!(found[1].message, "second\n\nbody");
        assert_eq!(found[1].path, "a.txt");
        assert!(found[1].commit_time > 0);
        assert!(!found[1].author.is_empty());
        assert!(matches!(
            updates[1],
            SelectionUpdate::Revisions {
                stream: 7,
                local_time: Some(5),
                ..
            }
        ));
        assert_eq!(
            blocks(&updates),
            [
                lines(&["a", "b3"]),
                lines(&["a", "b2"]),
                lines(&["a", "b2"]),
                lines(&["a", "b"])
            ]
        );
        let indexes: Vec<usize> = updates
            .iter()
            .filter_map(|update| match update {
                SelectionUpdate::Block { index, stream, .. } if *stream == 7 => Some(*index),
                _ => None,
            })
            .collect();
        assert_eq!(indexes, [0, 1, 2, 3]);
    }

    // `log.showSignature` on: the signature check stays out of the messages and of the history.
    #[test]
    fn signatures_stay_out_of_the_messages() {
        let repo = TempRepo::new();
        repo.sign_commits();
        let first = repo.commit("a.txt", b"a\n", "first\n\nbody");
        let second = repo.commit("a.txt", b"a\nb\n", "second");
        assert!(repo
            .git(&["log", "-1", "--format=%s"])
            .contains("No signature"));

        let found = revisions(&streamed(&repo, &request("a.txt", 0, 0, "a\nb\n")));

        let messages: Vec<(&str, &str)> = found
            .iter()
            .map(|revision| (revision.hash.as_str(), revision.message.as_str()))
            .collect();
        assert_eq!(
            messages,
            [
                (second.as_str(), "second"),
                (first.as_str(), "first\n\nbody")
            ]
        );
    }

    #[test]
    fn a_range_that_disappears_is_empty_in_every_older_revision() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"a\nc\n", "first");
        repo.commit("a.txt", b"a\nb\nc\n", "adds b");
        repo.commit("a.txt", b"a\nb\nc\nd\n", "adds d");
        let updates = streamed(&repo, &request("a.txt", 1, 1, "a\nb\nc\nd\n"));
        assert_eq!(
            blocks(&updates),
            [lines(&["b"]), lines(&["b"]), lines(&["b"]), None]
        );
    }

    // Through the index with the exact bytes: APFS keeps one name on disk for the NFD and NFC forms.
    fn index_add(repo: &TempRepo, blob: &str, name: &str) {
        let info = format!("100644,{blob},{name}");
        let exact = ["-c", "core.precomposeunicode=false", "update-index"];
        repo.git(&[&exact[..], &["--add", "--cacheinfo", &info]].concat());
    }

    fn commit_exact(repo: &TempRepo, name: &str, contents: &[u8], message: &str) -> String {
        let scratch = repo.path().join(".scratch");
        fs::write(&scratch, contents).unwrap();
        let blob = repo.git(&["hash-object", "-w", ".scratch"]);
        fs::remove_file(scratch).unwrap();
        index_add(repo, blob.trim(), name);
        repo.git(&["commit", "-q", "-m", message]);
        head(repo)
    }

    fn rename_exact(repo: &TempRepo, old: &str, new: &str, message: &str) -> String {
        let exact = ["-c", "core.precomposeunicode=false"];
        let blob = repo.git(&[&exact[..], &["rev-parse", &format!("HEAD:{old}")]].concat());
        repo.git(&[&exact[..], &["update-index", "--force-remove", "--", old]].concat());
        index_add(repo, blob.trim(), new);
        repo.git(&["commit", "-q", "-m", message]);
        head(repo)
    }

    // Review Focus 3: the decomposed `é`, a space and a glob character reach that file only, at
    // the path it had in each revision.
    #[test]
    fn a_renamed_file_is_read_at_each_revisions_path() {
        let repo = TempRepo::new();
        repo.git(&["config", "core.precomposeunicode", "true"]);
        let name = "cafe\u{301} [1].txt";
        let old = commit_exact(&repo, name, b"one\ntwo\nthree\n", "first");
        commit_exact(&repo, "caf\u{e9} [1].txt", b"other\n", "the composed name");
        let renamed = rename_exact(&repo, name, "new.txt", "rename");
        let updates = streamed(&repo, &request("new.txt", 1, 1, "one\ntwo\nthree\n"));
        let found = revisions(&updates);
        let paths: Vec<(&str, &str)> = found
            .iter()
            .map(|revision| (revision.hash.as_str(), revision.path.as_str()))
            .collect();
        assert_eq!(paths, [(renamed.as_str(), "new.txt"), (old.as_str(), name)]);
        assert_eq!(
            blocks(&updates),
            [lines(&["two"]), lines(&["two"]), lines(&["two"])]
        );
    }

    #[test]
    fn a_repository_without_commits_has_only_the_local_block() {
        let repo = TempRepo::new();
        fs::write(repo.path().join("a.txt"), "a\n").unwrap();
        let updates = streamed(&repo, &request("a.txt", 0, 0, "a\n"));
        assert_eq!(revisions(&updates), []);
        assert_eq!(blocks(&updates), [lines(&["a"])]);
    }

    #[test]
    fn a_cancelled_stream_stops_before_the_next_revision() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"a\n", "first");
        repo.commit("a.txt", b"a\nb\n", "second");
        repo.commit("a.txt", b"a\nb\nc\n", "third");
        let updates = RefCell::new(Vec::new());
        let result = selection_history(
            &mut real(repo.path()),
            &request("a.txt", 0, 0, "a\nb\nc\n"),
            None,
            &mut |update| updates.borrow_mut().push(update),
            &|| {
                let seen = blocks(&updates.borrow()).len();
                if seen >= 2 {
                    Err(GitError::Cancelled)
                } else {
                    Ok(())
                }
            },
        );
        assert_eq!(result, Err(GitError::Cancelled));
        assert_eq!(blocks(&updates.borrow()).len(), 2);
    }

    #[test]
    fn a_revision_that_cannot_be_read_ends_the_stream_with_its_error() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"a\n", "first");
        let mut calls = 0;
        let mut git = |args: &[&str]| {
            if args.contains(&"cat-file") {
                calls += 1;
                return Err(GitError::Io("broken blob".to_string()));
            }
            real(repo.path())(args)
        };
        let mut updates = Vec::new();
        let result = selection_history(
            &mut git,
            &request("a.txt", 0, 0, "a\n"),
            None,
            &mut |update| updates.push(update),
            &|| Ok(()),
        );
        assert_eq!(result, Err(GitError::Io("broken blob".to_string())));
        assert_eq!(calls, 1);
        assert_eq!(blocks(&updates).len(), 1);
    }

    // The window's close (`close`, from `request_close`, Esc and `repo_open`) releases what it
    // had: the stream it runs is cancelled, and no block of an older revision is read or sent
    // after it. Destroying the window itself needs an `AppHandle` and is not covered here.
    #[test]
    fn closing_the_window_cancels_the_stream() {
        let repo = TempRepo::new();
        for content in ["a\n", "a\nb\n", "a\nb\nc\n", "a\nb\nc\nd\n"] {
            repo.commit("a.txt", content.as_bytes(), content);
        }
        let runner = Runner::new(Arc::default(), Arc::default());
        let (updates, received) = mpsc::channel();
        let (resume, resumed) = mpsc::channel::<()>();
        let resumed = Mutex::new(resumed);
        let session = WindowSession::default();
        start_stream(
            &runner,
            &session.selection_running,
            "main",
            repo.path(),
            request("a.txt", 0, 0, "a\nb\nc\nd\n"),
            None,
            move |update| {
                let first = matches!(update, SelectionUpdate::Block { index: 1, .. });
                updates.send(update).unwrap();
                if first {
                    resumed.lock().unwrap().recv().unwrap();
                }
            },
        );
        let mut seen = Vec::new();
        while !matches!(seen.last(), Some(SelectionUpdate::Block { index: 1, .. })) {
            seen.push(received.recv_timeout(Duration::from_secs(30)).unwrap());
        }

        *locked(&session.selection_pending) = Some(request("b.txt", 0, 0, ""));
        release(&runner, &session);
        resume.send(()).unwrap();

        // The task ends, and drops its sender, without another block.
        seen.extend(received.iter());
        assert_eq!(blocks(&seen).len(), 2);
        assert_eq!(*locked(&session.selection_running), None);
        assert_eq!(*locked(&session.selection_pending), None);
    }

    // Window B's stream does not cancel window A's: each project window has its own slot.
    #[test]
    fn a_stream_of_another_window_does_not_cancel_this_one() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"a\n", "one");
        let runner = Runner::new(Arc::default(), Arc::default());
        let first = WindowSession::default();
        let second = WindowSession::default();
        for (session, owner) in [(&first, "main"), (&second, "main-2")] {
            start_stream(
                &runner,
                &session.selection_running,
                owner,
                repo.path(),
                request("a.txt", 0, 0, "a\n"),
                None,
                |_| {},
            );
        }
        let kept = locked(&first.selection_running).clone();
        assert_eq!(kept.map(|(owner, _)| owner), Some("main".to_string()));
        assert_eq!(
            locked(&second.selection_running)
                .clone()
                .map(|(owner, _)| owner),
            Some("main-2".to_string())
        );
        release(&runner, &second);
        assert!(locked(&first.selection_running).is_some());
    }
}
