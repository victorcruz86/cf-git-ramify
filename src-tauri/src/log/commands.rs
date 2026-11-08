use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, State};

use crate::commands::{blocking_api, current_repo, failed, repo_data_dir, ApiError, AppState};
use crate::git::{self, GitError};
use crate::log::collapse::{self, GraphTarget};
use crate::log::details::{self, ChangeGroup, ChangeMode, CommitDetails};
use crate::log::filter::{self, same_person, Highlight, Highlighter};
use crate::log::options;
use crate::log::print::{self, PrintRow};
use crate::log::recent::{self, RecentFilters};
use crate::log::refs::{self, GitRef, RefKind};
use crate::log::store::{self, CommitStore, Person};
use crate::log::view::{Permanent, View, ViewPatch, ViewSpec};
use crate::task::{Runner, TaskContext, TaskId, TaskKind};
use crate::worktree_diff::content_hash;

const FIRST_VIEW: u32 = 1;
const FIRST_TAB: u32 = 0;
const FILTERING: &str = "Applying Filters…";
const LOADING_CHANGES: &str = "Loading Commit Changes";
const CLIPBOARD: &str = "/usr/bin/pbcopy";
const CLIPBOARD_READER: &str = "/usr/bin/pbpaste";

// What does not change while the log is open. A task keeps this to work without holding
// `state.log` locked, which the scroll depends on.
struct LogData {
    store: CommitStore,
    permanent: Arc<Permanent>,
    me: Option<Person>,
    highlighter: Highlighter,
}

pub struct OpenRepo {
    data: Arc<LogData>,
    views: HashMap<u32, Arc<View>>,
    // Per tab, its view and the last task that opened one.
    tabs: HashMap<u32, u32>,
    pending: HashMap<u32, TaskId>,
    next_view: u32,
    // Queues the changes to a view, which are computed without holding the log locked.
    changing: Arc<Mutex<()>>,
    // From before reading the log: a change during the read shows up as a change.
    fingerprint: u64,
}

pub type SharedLog = Arc<Mutex<Option<OpenRepo>>>;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogSummary {
    pub view_id: u32,
    pub count: u32,
    pub collapsible: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewOpened {
    pub view_id: u32,
    pub summary: LogSummary,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphClicked {
    pub changed: bool,
    pub summary: LogSummary,
}

// Where `Go To Hash/Branch/Tag` goes, already decided by `resolveTarget`.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum JumpTarget {
    Ref(String),
    Hash(String),
}

// `hash` is missing if the commit does not exist. With `hash` and without `row`, it does not pass
// the view's filters.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct RowOf {
    pub row: Option<u32>,
    pub hash: Option<String>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct LogRef {
    pub name: String,
    pub kind: RefKind,
}

impl From<&GitRef> for LogRef {
    fn from(git_ref: &GitRef) -> Self {
        Self {
            name: git_ref.name.to_string(),
            kind: git_ref.kind,
        }
    }
}

// A label of the table: its name and a color per reference.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct LabelGroup {
    pub name: String,
    pub kinds: Vec<RefKind>,
}

// The two forms of `Compact References View`, with the tag names: the table options change
// without asking for the rows again.
#[derive(Debug, Serialize)]
pub struct RowLabels {
    pub compact: Vec<LabelGroup>,
    pub normal: Vec<LabelGroup>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogRow {
    pub hash: String,
    pub subject: String,
    pub author: String,
    pub time: i64,
    pub commit_time: i64,
    pub same_person: bool,
    // More than one: a merge, which Revert Commit does not support.
    pub parent_count: u32,
    pub refs: Vec<LogRef>,
    pub labels: RowLabels,
    pub highlight: Highlight,
    pub print: PrintRow,
}

fn config_value(repo: &Path, key: &str) -> Result<String, GitError> {
    match git::run(repo, &["config", "--get", key]) {
        Ok(output) => Ok(String::from_utf8_lossy(&output).trim().to_string()),
        Err(GitError::Failed { code: Some(1), .. }) => Ok(String::new()),
        Err(error) => Err(error),
    }
}

// `GitUserRegistry.readCurrentUser`.
fn current_user(repo: &Path) -> Result<Option<Person>, GitError> {
    let (name, email) = (
        config_value(repo, "user.name")?,
        config_value(repo, "user.email")?,
    );
    Ok((!name.is_empty() || !email.is_empty()).then(|| Person {
        name: name.into(),
        email: email.into(),
    }))
}

pub fn open_repo(path: &Path) -> Result<OpenRepo, ApiError> {
    let fingerprint = store::refs_fingerprint(path)?;
    let store = store::load(path)?;
    let me = current_user(path)?;
    let permanent = Arc::new(Permanent::from_store(&store, store.refs()));
    let view = View::new(permanent.clone(), &ViewSpec::default());
    Ok(OpenRepo {
        data: Arc::new(LogData {
            highlighter: Highlighter::new(&store, me.as_ref()),
            store,
            permanent,
            me,
        }),
        views: HashMap::from([(FIRST_VIEW, Arc::new(view))]),
        tabs: HashMap::from([(FIRST_TAB, FIRST_VIEW)]),
        pending: HashMap::new(),
        next_view: FIRST_VIEW + 1,
        changing: Arc::default(),
        fingerprint,
    })
}

pub fn summary(repo: &OpenRepo) -> LogSummary {
    view_summary(FIRST_VIEW, &repo.views[&FIRST_VIEW])
}

fn view_summary(view_id: u32, view: &View) -> LogSummary {
    LogSummary {
        view_id,
        count: view.graph.len(),
        collapsible: view.collapsible(),
    }
}

fn view_of(repo: &OpenRepo, view_id: u32) -> Result<Arc<View>, ApiError> {
    repo.views
        .get(&view_id)
        .cloned()
        .ok_or_else(|| failed(format!("View {view_id} is not open")))
}

// The first BEK order takes a while, and the scroll depends on `log`: the change is made on a
// copy with `log` unlocked. Meanwhile the previous view keeps being read.
fn change_view<T>(
    log: &SharedLog,
    view_id: u32,
    change: impl FnOnce(&mut View) -> T,
) -> Result<(LogSummary, T), ApiError> {
    let queue = with_log(log, |repo| Ok(repo.changing.clone()))?;
    let _turn = queue.lock().unwrap();
    let current = with_log(log, |repo| view_of(repo, view_id))?;
    let mut copy = View::clone(&current);
    let result = change(&mut copy);
    let summary = view_summary(view_id, &copy);
    with_log(log, |repo| match repo.views.get_mut(&view_id) {
        // Without it, or with another, the log was reloaded and this change is no longer valid.
        Some(view) if Arc::ptr_eq(view, &current) => {
            *view = Arc::new(copy);
            Ok(())
        }
        _ => Err(failed(format!("View {view_id} is not open"))),
    })?;
    Ok((summary, result))
}

pub fn set_view(log: &SharedLog, view_id: u32, patch: &ViewPatch) -> Result<LogSummary, ApiError> {
    change_view(log, view_id, |view| view.apply(patch)).map(|(summary, ())| summary)
}

pub fn graph_click(
    log: &SharedLog,
    view_id: u32,
    target: GraphTarget,
) -> Result<GraphClicked, ApiError> {
    change_view(log, view_id, |view| collapse::toggle(view, target))
        .map(|(summary, changed)| GraphClicked { changed, summary })
}

fn row_labels(refs_of_commit: &[&GitRef], all: &refs::Refs) -> RowLabels {
    let labels = |compact| {
        refs::groups(refs_of_commit, all, compact, true)
            .into_iter()
            .map(|group| LabelGroup {
                name: group.name,
                kinds: group.refs.iter().map(|git_ref| git_ref.kind).collect(),
            })
            .collect()
    };
    RowLabels {
        compact: labels(true),
        normal: labels(false),
    }
}

pub fn rows_in(repo: &OpenRepo, view_id: u32, start: u32, count: u32) -> Vec<LogRow> {
    let Some(view) = repo.views.get(&view_id) else {
        return Vec::new();
    };
    let end = start.saturating_add(count).min(view.graph.len());
    if start >= end {
        return Vec::new();
    }
    let printed = print::elements_in_rows(view, start, end);
    let store = &repo.data.store;
    let highlighter = &repo.data.highlighter;
    let rules = highlighter.rules(store, &view.spec.filter);
    (start..end)
        .zip(printed)
        .map(|(row, print)| {
            let index = view.graph.rows[row as usize];
            let commit = store.get(index);
            let author = store.people().get(commit.author);
            let committer = store.people().get(commit.committer);
            let refs_of_commit = store.refs().of_commit(index);
            LogRow {
                hash: commit.hash.to_string(),
                subject: commit.subject.to_string(),
                author: author.name.to_string(),
                time: commit.author_time,
                commit_time: commit.commit_time,
                same_person: same_person(author, committer),
                parent_count: commit.parents.len() as u32,
                labels: row_labels(&refs_of_commit, store.refs()),
                refs: refs_of_commit.into_iter().map(LogRef::from).collect(),
                highlight: highlighter.of(store, index, rules),
                print,
            }
        })
        .collect()
}

impl OpenRepo {
    // The new one replaces that of its tab, which the frontend has kept showing until now.
    fn install(&mut self, tab: u32, view: View) -> ViewOpened {
        let view_id = self.next_view;
        self.next_view += 1;
        let summary = view_summary(view_id, &view);
        self.views.insert(view_id, Arc::new(view));
        if let Some(old) = self.tabs.insert(tab, view_id) {
            self.views.remove(&old);
        }
        ViewOpened { view_id, summary }
    }
}

fn open_view(
    context: &TaskContext,
    log: &SharedLog,
    data: Arc<LogData>,
    tab: u32,
    spec: &ViewSpec,
) -> Result<ViewOpened, GitError> {
    let selection = filter::select(
        &data.store,
        data.me.as_ref(),
        &spec.filter,
        &mut |args: &[String]| {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            context.git(&args).map(|output| output.stdout)
        },
    )?;
    context.check_cancelled()?;
    let view = View::filtered(data.permanent.clone(), spec, selection);
    let mut log = log.lock().unwrap();
    // The rows of a view are indices into its store: one from a reloaded log is not valid.
    let repo = log
        .as_mut()
        .filter(|repo| Arc::ptr_eq(&repo.data, &data))
        .ok_or(GitError::Cancelled)?;
    context.check_cancelled()?;
    Ok(repo.install(tab, view))
}

// With `log` locked from cancelling until the new task is recorded: that way, the previous one
// either sees the cancellation before installing its view, or installs it before the new one.
pub fn start_view(
    log: &SharedLog,
    runner: &Runner,
    repo: &Path,
    tab: u32,
    spec: ViewSpec,
) -> Result<TaskId, ApiError> {
    let mut guard = log.lock().unwrap();
    let open = guard
        .as_mut()
        .ok_or_else(|| failed("The log is not open".to_string()))?;
    if let Some(previous) = open.pending.get(&tab) {
        runner.cancel(*previous);
    }
    let data = open.data.clone();
    let shared = log.clone();
    let id = runner.start(
        repo,
        TaskKind::Read,
        FILTERING.to_string(),
        move |context| open_view(context, &shared, data, tab, &spec),
    );
    open.pending.insert(tab, id);
    Ok(id)
}

pub fn close_view(repo: &mut OpenRepo, tab: u32) {
    if let Some(view) = repo.tabs.remove(&tab) {
        repo.views.remove(&view);
    }
}

// By hash, its row in the view. Rows are rarely requested: an index would always cost memory.
fn find_rows(data: &LogData, view: &View, hashes: &[String]) -> Vec<Option<u32>> {
    let commits = data.store.find(hashes.iter().map(String::as_str));
    let wanted: HashSet<u32> = commits.values().copied().collect();
    let rows: HashMap<u32, u32> = view
        .graph
        .rows
        .iter()
        .enumerate()
        .filter(|(_, commit)| wanted.contains(*commit))
        .map(|(row, commit)| (*commit, row as u32))
        .collect();
    hashes
        .iter()
        .map(|hash| rows.get(commits.get(hash.as_str())?).copied())
        .collect()
}

// `VcsLogNavigationUtil.getCommitRow`. A prefix can match several commits: one that is visible
// wins.
fn row_of(store: &CommitStore, view: &View, target: &JumpTarget) -> RowOf {
    let commits = commits_of(store, target);
    let wanted: HashSet<u32> = commits.iter().copied().collect();
    let rows = &view.graph.rows;
    let row = rows.iter().position(|commit| wanted.contains(commit));
    let commit = row.map(|row| rows[row]).or(commits.first().copied());
    RowOf {
        row: row.map(|row| row as u32),
        hash: commit.map(|commit| store.get(commit).hash.to_string()),
    }
}

fn commits_of(store: &CommitStore, target: &JumpTarget) -> Vec<u32> {
    match target {
        JumpTarget::Ref(name) => {
            let refs = store.refs();
            let mut named: Vec<&GitRef> = refs
                .all
                .iter()
                .filter(|item| *item.name == **name)
                .collect();
            refs.sort_labels(&mut named);
            named.first().map(|item| item.commit).into_iter().collect()
        }
        JumpTarget::Hash(prefix) => (0..store.len())
            .filter(|commit| {
                let start = store.get(*commit).hash.get(..prefix.len());
                start.is_some_and(|start| start.eq_ignore_ascii_case(prefix))
            })
            .collect(),
    }
}

// `RowInfo.getAdjacentRows`.
fn adjacent_rows(view: &View, row: u32, parent: bool) -> Vec<u32> {
    if row >= view.graph.len() {
        return Vec::new();
    }
    let mut rows: Vec<u32> = if parent {
        let edges = view.graph.downs.of(row).iter();
        edges.map(|edge| edge.down).collect()
    } else {
        let edges = view.graph.ups.of(row).iter();
        edges.map(|edge| edge.up).collect()
    };
    rows.sort_unstable();
    rows
}

pub fn start_selected_changes(
    runner: &Runner,
    repo: &Path,
    hashes: Vec<String>,
    limit: Option<u32>,
) -> TaskId {
    runner.start(
        repo,
        TaskKind::Read,
        LOADING_CHANGES.to_string(),
        move |context| {
            details::selected_changes(&hashes, limit, |args, input| {
                Ok(context.git_with_input(args, input)?.stdout)
            })
        },
    )
}

pub fn refs_changed(repo: &Path, known: u64) -> Result<bool, GitError> {
    Ok(store::refs_fingerprint(repo)? != known)
}

pub fn current_branch(repo: &OpenRepo) -> Option<String> {
    let refs = repo.data.store.refs();
    refs.current_name().map(str::to_string)
}

pub fn users_of(repo: &OpenRepo) -> Vec<Person> {
    repo.data.store.people().all().to_vec()
}

// In the order of the labels, which is the one `Go To Hash/Branch/Tag` uses.
pub fn refs_of(repo: &OpenRepo) -> Vec<LogRef> {
    let refs = repo.data.store.refs();
    let mut sorted: Vec<&GitRef> = refs.all.iter().collect();
    refs.sort_labels(&mut sorted);
    sorted.into_iter().map(LogRef::from).collect()
}

// `pbcopy` reads in the environment's encoding, and an app opened from Finder has none.
// The clipboard of Copy as Patch and Create Patch.
pub(crate) fn copy_to_clipboard(text: &str) -> io::Result<()> {
    copy_with(CLIPBOARD, text)
}

fn copy_with(program: &str, text: &str) -> io::Result<()> {
    let mut child = Command::new(program)
        .env("LC_CTYPE", "UTF-8")
        .stdin(Stdio::piped())
        .spawn()?;
    if let Some(mut input) = child.stdin.take() {
        input.write_all(text.as_bytes())?;
    }
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("{program}: {status}")))
    }
}

// `pbpaste` writes in the environment's encoding too. What is not UTF-8 is shown replaced, as the
// text it is: whoever reads it (Apply Patch from Clipboard) only takes text.
fn paste_with(program: &str) -> io::Result<String> {
    let output = Command::new(program)
        .env("LC_CTYPE", "UTF-8")
        .stdin(Stdio::null())
        .output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(io::Error::other(format!("{program}: {}", output.status)))
    }
}

#[tauri::command]
pub async fn log_open(state: State<'_, AppState>) -> Result<LogSummary, ApiError> {
    let path = current_repo(&state)?;
    let open = tauri::async_runtime::spawn_blocking(move || open_repo(&path))
        .await
        .map_err(|error| failed(error.to_string()))??;
    let result = summary(&open);
    *state.log.lock().unwrap() = Some(open);
    Ok(result)
}

#[tauri::command]
pub fn log_rows(
    view_id: u32,
    start: u32,
    count: u32,
    state: State<'_, AppState>,
) -> Result<Vec<LogRow>, ApiError> {
    let log = state.log.lock().unwrap();
    Ok(log
        .as_ref()
        .map(|log| rows_in(log, view_id, start, count))
        .unwrap_or_default())
}

fn with_log<T>(
    log: &SharedLog,
    action: impl FnOnce(&mut OpenRepo) -> Result<T, ApiError>,
) -> Result<T, ApiError> {
    let mut log = log.lock().unwrap();
    let repo = log
        .as_mut()
        .ok_or_else(|| failed("The log is not open".to_string()))?;
    action(repo)
}

#[tauri::command]
pub async fn log_view_set(
    view_id: u32,
    patch: ViewPatch,
    state: State<'_, AppState>,
) -> Result<LogSummary, ApiError> {
    let log = state.log.clone();
    blocking_api(move || set_view(&log, view_id, &patch)).await
}

#[tauri::command]
pub async fn log_graph_click(
    view_id: u32,
    target: GraphTarget,
    state: State<'_, AppState>,
) -> Result<GraphClicked, ApiError> {
    let log = state.log.clone();
    blocking_api(move || graph_click(&log, view_id, target)).await
}

// It only reads: hovering does not queue behind the view's changes.
#[tauri::command]
pub async fn log_graph_hover(
    view_id: u32,
    target: GraphTarget,
    state: State<'_, AppState>,
) -> Result<Option<Vec<u32>>, ApiError> {
    let view = with_log(&state.log, |repo| view_of(repo, view_id))?;
    blocking_api(move || Ok(collapse::hover(&view, target))).await
}

#[tauri::command]
pub async fn log_view_open(
    tab: u32,
    spec: ViewSpec,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    let path = current_repo(&state)?;
    start_view(&state.log, &state.tasks, &path, tab, spec)
}

#[tauri::command]
pub async fn log_users(state: State<'_, AppState>) -> Result<Vec<Person>, ApiError> {
    with_log(&state.log, |repo| Ok(users_of(repo)))
}

#[tauri::command]
pub async fn log_refs(state: State<'_, AppState>) -> Result<Vec<LogRef>, ApiError> {
    with_log(&state.log, |repo| Ok(refs_of(repo)))
}

#[tauri::command]
pub async fn log_current_branch(state: State<'_, AppState>) -> Result<Option<String>, ApiError> {
    with_log(&state.log, |repo| Ok(current_branch(repo)))
}

#[tauri::command]
pub async fn log_view_close(tab: u32, state: State<'_, AppState>) -> Result<(), ApiError> {
    with_log(&state.log, |repo| {
        close_view(repo, tab);
        Ok(())
    })
}

#[tauri::command]
pub async fn log_find_rows(
    view_id: u32,
    hashes: Vec<String>,
    state: State<'_, AppState>,
) -> Result<Vec<Option<u32>>, ApiError> {
    let (data, view) = with_log(&state.log, |repo| {
        Ok((repo.data.clone(), view_of(repo, view_id)?))
    })?;
    blocking_api(move || Ok(find_rows(&data, &view, &hashes))).await
}

#[tauri::command]
pub async fn log_row_of(
    view_id: u32,
    target: JumpTarget,
    state: State<'_, AppState>,
) -> Result<RowOf, ApiError> {
    let (data, view) = with_log(&state.log, |repo| {
        Ok((repo.data.clone(), view_of(repo, view_id)?))
    })?;
    blocking_api(move || Ok(row_of(&data.store, &view, &target))).await
}

#[tauri::command]
pub async fn log_adjacent_rows(
    view_id: u32,
    row: u32,
    parent: bool,
    state: State<'_, AppState>,
) -> Result<Vec<u32>, ApiError> {
    let view = with_log(&state.log, |repo| view_of(repo, view_id))?;
    Ok(adjacent_rows(&view, row, parent))
}

#[tauri::command]
pub async fn clipboard_write(text: String) -> Result<(), ApiError> {
    blocking_api(move || copy_with(CLIPBOARD, &text).map_err(|error| failed(error.to_string())))
        .await
}

// What Apply Patch from Clipboard needs of the clipboard. `length` is in UTF-16 units, as Java's
// `String.length()` (0: no text). `patch` is the text, only when `PatchReader.isPatchContent` says
// it is a patch, so that a large text that is not one does not travel. With a `limit`
// (`ClipboardAnalyzeListener.MAX_SIZE`), a text that long is not looked at, and a shorter one
// comes with its hash, which the on-the-fly check remembers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardPatch {
    pub length: usize,
    pub hash: Option<String>,
    pub patch: Option<String>,
}

fn clipboard_patch(text: String, limit: Option<usize>) -> ClipboardPatch {
    let length = text.encode_utf16().count();
    let within = limit.is_none_or(|limit| length < limit);
    let hash = (length > 0 && within && limit.is_some()).then(|| content_hash(text.as_bytes()));
    let patch = (length > 0 && within && crate::patch::is_patch(text.as_bytes())).then_some(text);
    ClipboardPatch {
        length,
        hash,
        patch,
    }
}

#[tauri::command]
pub async fn patch_clipboard(limit: Option<usize>) -> Result<ClipboardPatch, ApiError> {
    blocking_api(move || {
        let text = paste_with(CLIPBOARD_READER).map_err(|error| failed(error.to_string()))?;
        Ok(clipboard_patch(text, limit))
    })
    .await
}

// Without an open log there is nothing to refresh.
#[tauri::command]
pub async fn log_refs_changed(state: State<'_, AppState>) -> Result<bool, ApiError> {
    let known = state
        .log
        .lock()
        .unwrap()
        .as_ref()
        .map(|repo| repo.fingerprint);
    let Some(known) = known else {
        return Ok(false);
    };
    let path = current_repo(&state)?;
    blocking_api(move || Ok(refs_changed(&path, known)?)).await
}

// The push dialog shows commits even if the log was never opened. What the store contributes
// (references, links and parent subjects) is an extra: without it, it simply does not appear.
fn with_store<T>(
    log: &SharedLog,
    work: impl FnOnce(&CommitStore) -> T + Send + 'static,
) -> impl FnOnce() -> T + Send + 'static
where
    T: Send + 'static,
{
    let data = log.lock().unwrap().as_ref().map(|repo| repo.data.clone());
    move || match data {
        Some(data) => work(&data.store),
        None => work(&CommitStore::default()),
    }
}

#[tauri::command]
pub async fn commit_details(
    hashes: Vec<String>,
    state: State<'_, AppState>,
) -> Result<Vec<CommitDetails>, ApiError> {
    let path = current_repo(&state)?;
    let read = with_store(&state.log, move |store| {
        Ok(details::commit_details(&path, store, &hashes)?)
    });
    blocking_api(read).await
}

#[tauri::command]
pub async fn branches_containing(
    hash: String,
    state: State<'_, AppState>,
) -> Result<Vec<String>, ApiError> {
    let data = with_log(&state.log, |repo| Ok(repo.data.clone()))?;
    blocking_api(move || {
        Ok(details::branches_containing(
            &data.store,
            &data.permanent,
            &hash,
        ))
    })
    .await
}

#[tauri::command]
pub async fn commit_changes_of(
    hash: String,
    mode: ChangeMode,
    state: State<'_, AppState>,
) -> Result<Vec<ChangeGroup>, ApiError> {
    let path = current_repo(&state)?;
    let read = with_store(&state.log, move |store| {
        Ok(details::commit_changes(&path, store, &hash, mode)?)
    });
    blocking_api(read).await
}

#[tauri::command]
pub async fn commit_changes_start(
    hashes: Vec<String>,
    limit: Option<u32>,
    state: State<'_, AppState>,
) -> Result<TaskId, ApiError> {
    let path = current_repo(&state)?;
    Ok(start_selected_changes(&state.tasks, &path, hashes, limit))
}

#[tauri::command]
pub async fn log_options_get(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Value, ApiError> {
    Ok(options::read(&repo_data_dir(&app, &state)?))
}

#[tauri::command]
pub async fn log_options_set(
    options: Value,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    self::options::write(&repo_data_dir(&app, &state)?, &options)
        .map_err(|error| failed(error.to_string()))
}

#[tauri::command]
pub async fn log_recent_filters(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<RecentFilters, ApiError> {
    Ok(recent::all(&repo_data_dir(&app, &state)?))
}

#[tauri::command]
pub async fn log_remember_filter(
    filter: String,
    values: Vec<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<RecentFilters, ApiError> {
    recent::remember(&repo_data_dir(&app, &state)?, &filter, &values)
        .map_err(|error| failed(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::RepoLocks;
    use crate::console::Console;
    use crate::log::filter::{LogFilter, TextFilter};
    use crate::log::view::SortType;
    use crate::task::{TaskEvent, TaskState};
    use crate::test_support::TempRepo;
    use std::cell::RefCell;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc::{self, Receiver};
    use std::thread;
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    // Two branches with one commit each, and the merge.
    fn repo_with_merge() -> TempRepo {
        let repo = TempRepo::new();
        repo.commit_dated("a.txt", "base", "2026-01-01T12:00:00");
        repo.git(&["checkout", "-q", "-b", "other"]);
        repo.commit_dated("b.txt", "branch", "2026-01-02T12:00:00");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit_dated("c.txt", "main", "2026-01-03T12:00:00");
        repo.merge_dated("other", "merge", "2026-01-04T12:00:00");
        repo.git(&["branch", "-D", "other"]);
        repo
    }

    #[test]
    fn view_set_changes_the_row_order() {
        let repo = repo_with_merge();
        let log = shared(open_repo(repo.path()).unwrap());
        assert_eq!(shared_rows(&log, 1, 1)[0].subject, "main");
        let patch = ViewPatch {
            sort: Some(SortType::Standard),
            ..ViewPatch::default()
        };

        let summary = set_view(&log, FIRST_VIEW, &patch).unwrap();

        assert_eq!((summary.view_id, summary.count), (FIRST_VIEW, 4));
        assert_eq!(shared_rows(&log, 1, 1)[0].subject, "branch");
    }

    #[test]
    fn view_set_collapse_returns_the_visible_rows() {
        let repo = repo_with_merge();
        let log = shared(open_repo(repo.path()).unwrap());
        let patch = ViewPatch {
            collapsed: Some(true),
            ..ViewPatch::default()
        };

        let summary = set_view(&log, FIRST_VIEW, &patch).unwrap();

        assert_eq!(summary.count, 2);
        assert_eq!(shared_rows(&log, 0, 10).len(), 2);
    }

    fn store_of(repo: &TempRepo) -> CommitStore {
        store::load(repo.path()).unwrap()
    }

    fn view_with(store: &CommitStore, spec: &ViewSpec) -> View {
        View::build(store, store.refs(), spec)
    }

    fn hash_of(repo: &TempRepo, revision: &str) -> String {
        let output = git::run(repo.path(), &["rev-parse", revision]).unwrap();
        String::from_utf8_lossy(&output).trim().to_string()
    }

    #[test]
    fn row_of_a_branch() {
        let repo = repo_with_merge();
        repo.git(&["branch", "old", "HEAD~1"]);
        let store = store_of(&repo);
        let view = view_with(&store, &ViewSpec::default());

        let found = row_of(&store, &view, &JumpTarget::Ref("old".to_string()));

        assert_eq!(found.row, Some(1));
        assert_eq!(found.hash, Some(hash_of(&repo, "old")));
    }

    #[test]
    fn row_of_a_hash_prefix_ignoring_case() {
        let repo = repo_with_merge();
        let hash = hash_of(&repo, "HEAD~1");
        let store = store_of(&repo);
        let view = view_with(&store, &ViewSpec::default());

        let prefix = hash[..8].to_uppercase();
        let found = row_of(&store, &view, &JumpTarget::Hash(prefix));

        assert_eq!(found.row, Some(1));
        assert_eq!(found.hash, Some(hash));
    }

    #[test]
    fn commit_that_does_not_exist() {
        let repo = repo_with_merge();
        let store = store_of(&repo);
        let view = view_with(&store, &ViewSpec::default());

        let found = row_of(&store, &view, &JumpTarget::Hash("0000".to_string()));

        assert_eq!(
            found,
            RowOf {
                row: None,
                hash: None
            }
        );
    }

    #[test]
    fn commit_that_exists_but_is_not_visible() {
        let repo = repo_with_merge();
        let hash = hash_of(&repo, "HEAD~1");
        let store = store_of(&repo);
        let spec = ViewSpec {
            collapsed: true,
            ..ViewSpec::default()
        };
        let view = view_with(&store, &spec);

        let found = row_of(&store, &view, &JumpTarget::Hash(hash.clone()));

        assert_eq!(found.row, None);
        assert_eq!(found.hash, Some(hash));
    }

    #[test]
    fn target_comes_from_the_frontend_with_its_kind() {
        let target: JumpTarget =
            serde_json::from_str(r#"{ "kind": "hash", "value": "abcd" }"#).unwrap();
        assert!(matches!(target, JumpTarget::Hash(value) if value == "abcd"));
        let target: JumpTarget =
            serde_json::from_str(r#"{ "kind": "ref", "value": "main" }"#).unwrap();
        assert!(matches!(target, JumpTarget::Ref(value) if value == "main"));
    }

    #[test]
    fn rows_of_parents_and_children_sorted() {
        let repo = repo_with_merge();
        let store = store_of(&repo);
        let view = view_with(&store, &ViewSpec::default());

        // merge, main, branch, base: the merge has two parents and the base two children.
        assert_eq!(adjacent_rows(&view, 0, true), [1, 2]);
        assert_eq!(adjacent_rows(&view, 3, false), [1, 2]);
        assert!(adjacent_rows(&view, 0, false).is_empty());
        assert!(adjacent_rows(&view, 3, true).is_empty());
        assert!(adjacent_rows(&view, 99, true).is_empty());
    }

    #[test]
    fn current_branch_of_the_log() {
        let repo = repo_with_merge();
        assert_eq!(
            current_branch(&open_repo(repo.path()).unwrap()).as_deref(),
            Some("main")
        );

        repo.git(&["checkout", "-q", "--detach"]);

        assert_eq!(current_branch(&open_repo(repo.path()).unwrap()), None);
    }

    #[test]
    fn references_in_label_order() {
        let repo = repo_with_merge();
        repo.git(&["tag", "a-tag", "HEAD~1"]);
        repo.git(&["branch", "z-branch", "HEAD~1"]);
        let open = open_repo(repo.path()).unwrap();

        let names: Vec<String> = refs_of(&open).into_iter().map(|item| item.name).collect();

        assert_eq!(names, ["HEAD", "main", "z-branch", "a-tag"]);
    }

    #[test]
    fn copy_passes_the_text_as_utf8() {
        let dir = TempDir::new().unwrap();
        let program = dir.path().join("pbcopy");
        let out = dir.path().join("output");
        let script = format!(
            "#!/bin/sh\nprintf '%s:' \"$LC_CTYPE\" > '{0}'\ncat >> '{0}'\n",
            out.display()
        );
        fs::write(&program, script).unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();

        copy_with(program.to_str().unwrap(), "año\nsecond").unwrap();

        assert_eq!(fs::read_to_string(out).unwrap(), "UTF-8:año\nsecond");
    }

    #[test]
    fn copy_reports_failure() {
        let dir = TempDir::new().unwrap();
        let program = dir.path().join("pbcopy");
        fs::write(&program, "#!/bin/sh\nexit 3\n").unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();

        assert!(copy_with(program.to_str().unwrap(), "x").is_err());
    }

    #[test]
    fn paste_with_returns_utf8() {
        let dir = TempDir::new().unwrap();
        let program = dir.path().join("pbpaste");
        fs::write(
            &program,
            "#!/bin/sh\nprintf '%s:año\\nsecond\\n' \"$LC_CTYPE\"\n",
        )
        .unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();

        let text = paste_with(program.to_str().unwrap()).unwrap();

        assert_eq!(text, "UTF-8:año\nsecond\n");
    }

    #[test]
    fn paste_with_reports_failure() {
        let dir = TempDir::new().unwrap();
        let program = dir.path().join("pbpaste");
        fs::write(&program, "#!/bin/sh\nexit 3\n").unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();

        assert!(paste_with(program.to_str().unwrap()).is_err());
    }

    // Apply Patch from Clipboard: the text travels only when `PatchReader.isPatchContent` says it
    // is a patch. Its length is in UTF-16 units, as `String.length()`. With a limit (the
    // on-the-fly check's 100 KB), a text that long is not looked at, and a shorter one comes with
    // its hash, so that the check skips a text it has already seen.
    #[test]
    fn the_clipboard_patch_is_given_only_when_it_is_one() {
        let patch = "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-a\n+b\n";

        let found = clipboard_patch(patch.to_string(), None);
        assert_eq!(found.patch.as_deref(), Some(patch));
        assert_eq!(found.length, patch.len());
        assert_eq!(found.hash, None);
        let prose = clipboard_patch("añ😀 just words".to_string(), Some(100));
        assert_eq!(prose.patch, None);
        assert_eq!(prose.length, 15);
        assert_eq!(prose.hash, Some(content_hash("añ😀 just words".as_bytes())));
        assert_eq!(clipboard_patch(String::new(), Some(100)).hash, None);
        assert_eq!(clipboard_patch(String::new(), None).length, 0);
        let over = clipboard_patch(patch.to_string(), Some(patch.len()));
        assert_eq!((over.patch, over.hash), (None, None));
        let under = clipboard_patch(patch.to_string(), Some(patch.len() + 1));
        assert_eq!(under.patch.as_deref(), Some(patch));
        assert_eq!(under.hash, Some(content_hash(patch.as_bytes())));
    }

    // Review Focus 3: 20 MB of text that is not a patch answers at once. It takes about 0.1 s in a
    // debug build on a quiet machine; the bound is 5 s so that a suite under load does not fail it,
    // while a freeze or a quadratic scan (minutes on 20 MB) still does.
    #[test]
    fn a_20_mb_clipboard_that_is_not_a_patch_answers_at_once() {
        let line = "--- this line looks like a header but nothing follows it, with words\n";
        let text = line.repeat(20 * 1024 * 1024 / line.len() + 1);
        assert!(text.len() >= 20 * 1024 * 1024);
        let length = text.len();

        let started = std::time::Instant::now();
        let found = clipboard_patch(text, None);

        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(found.patch, None);
        assert_eq!(found.length, length);
    }

    #[test]
    fn graph_click_collapses_the_row() {
        let repo = repo_with_merge();
        let log = shared(open_repo(repo.path()).unwrap());

        let clicked = graph_click(&log, FIRST_VIEW, GraphTarget::Node { row: 0 }).unwrap();

        assert!(clicked.changed);
        assert_eq!(clicked.summary.count, 2);
    }

    #[test]
    fn graph_click_says_if_nothing_changed() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        let log = shared(open_repo(repo.path()).unwrap());

        let clicked = graph_click(&log, FIRST_VIEW, GraphTarget::Node { row: 0 }).unwrap();

        assert!(!clicked.changed);
        assert_eq!(clicked.summary.count, 1);
    }

    #[test]
    fn changing_a_view_does_not_hold_the_log() {
        let repo = repo_with_merge();
        let log = shared(open_repo(repo.path()).unwrap());

        change_view(&log, FIRST_VIEW, |_| {
            assert!(
                log.try_lock().is_ok(),
                "the computation holds the log locked"
            );
        })
        .unwrap();
    }

    #[test]
    fn two_simultaneous_changes_do_not_overwrite_each_other() {
        let repo = repo_with_merge();
        let log = shared(open_repo(repo.path()).unwrap());
        let (entered, wait) = mpsc::channel();
        let other = log.clone();
        let first = thread::spawn(move || {
            change_view(&other, FIRST_VIEW, |view| {
                entered.send(()).unwrap();
                thread::sleep(Duration::from_millis(100));
                view.apply(&ViewPatch {
                    long_edges: Some(true),
                    ..ViewPatch::default()
                });
            })
            .unwrap();
        });
        wait.recv().unwrap();

        let patch = ViewPatch {
            sort: Some(SortType::Standard),
            ..ViewPatch::default()
        };
        set_view(&log, FIRST_VIEW, &patch).unwrap();
        first.join().unwrap();

        let guard = log.lock().unwrap();
        let view = &guard.as_ref().unwrap().views[&FIRST_VIEW];
        assert!(view.spec.long_edges);
        assert_eq!(view.spec.sort, SortType::Standard);
    }

    #[test]
    fn a_change_arriving_after_reload_is_discarded() {
        let repo = repo_with_merge();
        let log = shared(open_repo(repo.path()).unwrap());
        let reopened = open_repo(repo.path()).unwrap();

        let error = change_view(&log, FIRST_VIEW, |_| {
            *log.lock().unwrap() = Some(reopened);
        })
        .err()
        .unwrap();

        assert_eq!(error.message, "View 1 is not open");
    }

    #[test]
    fn refresh_only_if_the_fingerprint_changes() {
        let repo = repo_with_merge();
        let open = open_repo(repo.path()).unwrap();
        assert!(!refs_changed(repo.path(), open.fingerprint).unwrap());

        fs::write(repo.path().join("a.txt"), "no commit").unwrap();
        assert!(!refs_changed(repo.path(), open.fingerprint).unwrap());

        repo.commit("a.txt", b"new", "new");
        assert!(refs_changed(repo.path(), open.fingerprint).unwrap());

        let reopened = open_repo(repo.path()).unwrap();
        assert!(!refs_changed(repo.path(), reopened.fingerprint).unwrap());
    }

    #[test]
    fn rows_of_some_hashes_in_the_view() {
        let repo = repo_with_merge();
        let open = open_repo(repo.path()).unwrap();
        let first = rows(&open, 0, 1)[0].hash.clone();
        let last = rows(&open, 3, 1)[0].hash.clone();
        let hashes = [last, "0".repeat(40), first];

        let found = find_rows(&open.data, &open.views[&FIRST_VIEW], &hashes);

        assert_eq!(found, [Some(3), None, Some(0)]);
    }

    #[test]
    fn a_collapsed_commit_has_no_row() {
        let repo = repo_with_merge();
        let log = shared(open_repo(repo.path()).unwrap());
        let hidden = shared_rows(&log, 1, 1)[0].hash.clone();
        let patch = ViewPatch {
            collapsed: Some(true),
            ..ViewPatch::default()
        };
        set_view(&log, FIRST_VIEW, &patch).unwrap();

        let guard = log.lock().unwrap();
        let open = guard.as_ref().unwrap();

        assert_eq!(
            find_rows(&open.data, &open.views[&FIRST_VIEW], &[hidden]),
            [None]
        );
    }

    #[test]
    fn closing_a_tab_releases_its_view() {
        let repo = repo_with_merge();
        let harness = Harness::new(&repo, "exit 1");
        let id = start_view(
            &harness.log,
            &harness.runner,
            repo.path(),
            7,
            by_branch("main"),
        )
        .unwrap();
        let view = harness.finished(id).value.unwrap()["viewId"]
            .as_u64()
            .unwrap() as u32;

        let mut log = harness.log.lock().unwrap();
        let open = log.as_mut().unwrap();
        close_view(open, 7);

        assert!(rows_in(open, view, 0, 10).is_empty());
        assert_eq!(rows_in(open, FIRST_VIEW, 0, 10).len(), 4);
    }

    #[test]
    fn unknown_view_is_an_error() {
        let repo = repo_with_merge();
        let log = shared(open_repo(repo.path()).unwrap());

        let error = set_view(&log, 99, &ViewPatch::default()).err().unwrap();
        assert_eq!(error.message, "View 99 is not open");
        assert!(graph_click(&log, 99, GraphTarget::Node { row: 0 }).is_err());
    }

    #[test]
    fn view_change_comes_from_the_frontend_in_camel_case() {
        let patch: ViewPatch =
            serde_json::from_str(r#"{"sort":"linear","longEdges":true,"collapsed":false}"#)
                .unwrap();

        assert_eq!(patch.sort, Some(SortType::Linear));
        assert_eq!(patch.long_edges, Some(true));
        assert_eq!(patch.collapsed, Some(false));
        let empty: ViewPatch = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, ViewPatch::default());
    }

    // A fake `git` for `Runner`: the test decides what it does without depending on the real one.
    struct Harness {
        runner: Runner,
        log: SharedLog,
        events: Receiver<TaskEvent>,
        seen: RefCell<Vec<TaskEvent>>,
        _dir: TempDir,
    }

    impl Harness {
        fn new(repo: &TempRepo, body: &str) -> Self {
            let dir = TempDir::new().unwrap();
            let script = dir.path().join("git");
            fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
            let runner = Runner::with(
                Arc::new(RepoLocks::default()),
                Arc::new(Console::default()),
                Some(script),
                Duration::from_secs(5),
            );
            let (send, events) = mpsc::channel();
            let send = Mutex::new(send);
            runner.listen(move |event| {
                let _ = send.lock().unwrap().send(event.clone());
            });
            Self {
                runner,
                log: Arc::new(Mutex::new(Some(open_repo(repo.path()).unwrap()))),
                events,
                seen: RefCell::default(),
                _dir: dir,
            }
        }

        fn finished(&self, id: TaskId) -> TaskEvent {
            loop {
                let known = self.seen.borrow().iter().position(|event| event.id == id);
                if let Some(position) = known {
                    return self.seen.borrow_mut().remove(position);
                }
                let event = self.events.recv_timeout(Duration::from_secs(20)).unwrap();
                if event.state != TaskState::Running {
                    self.seen.borrow_mut().push(event);
                }
            }
        }

        fn view_rows(&self, event: &TaskEvent) -> Vec<LogRow> {
            let view_id = event.value.as_ref().unwrap()["viewId"].as_u64().unwrap() as u32;
            let log = self.log.lock().unwrap();
            rows_in(log.as_ref().unwrap(), view_id, 0, 100)
        }
    }

    fn by_text(text: &str) -> ViewSpec {
        ViewSpec {
            filter: LogFilter {
                text: Some(TextFilter {
                    text: text.to_string(),
                    ..TextFilter::default()
                }),
                ..LogFilter::default()
            },
            ..ViewSpec::default()
        }
    }

    fn by_branch(name: &str) -> ViewSpec {
        ViewSpec {
            filter: LogFilter {
                branches: vec![name.to_string()],
                ..LogFilter::default()
            },
            ..ViewSpec::default()
        }
    }

    fn wait_for(path: &Path) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !path.exists() {
            assert!(Instant::now() < deadline, "the fake did not start");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn in_memory_branch_filter_does_not_launch_git() {
        let repo = repo_with_merge();
        repo.git(&["branch", "side", "main~1"]);
        let called = TempDir::new().unwrap();
        let marker = called.path().join("called");
        let harness = Harness::new(&repo, &format!("touch '{}'\nexit 1", marker.display()));

        let id = start_view(
            &harness.log,
            &harness.runner,
            repo.path(),
            0,
            by_branch("side"),
        )
        .unwrap();

        let event = harness.finished(id);
        assert_eq!(event.state, TaskState::Done, "{}", event.text);
        assert!(!marker.exists());
        assert_eq!(harness.view_rows(&event)[0].subject, "main");
    }

    #[test]
    fn changes_of_several_commits_are_one_command_and_the_hashes_go_through_stdin() {
        let repo = repo_with_merge();
        let files = TempDir::new().unwrap();
        let input = files.path().join("input");
        let calls = files.path().join("calls");
        let harness = Harness::new(
            &repo,
            &format!(
                "echo x >> '{}'\ncat > '{}'",
                calls.display(),
                input.display()
            ),
        );
        let hashes = vec!["a".repeat(40), "b".repeat(40)];

        let id = start_selected_changes(&harness.runner, repo.path(), hashes, Some(10));

        let event = harness.finished(id);
        assert_eq!(event.state, TaskState::Done, "{}", event.text);
        assert_eq!(
            fs::read_to_string(input).unwrap(),
            format!("{}\n{}\n", "a".repeat(40), "b".repeat(40))
        );
        assert_eq!(fs::read_to_string(calls).unwrap().lines().count(), 1);
    }

    #[test]
    fn the_view_arrives_with_its_summary() {
        let repo = repo_with_merge();
        let harness = Harness::new(&repo, "exit 1");

        let id = start_view(
            &harness.log,
            &harness.runner,
            repo.path(),
            0,
            by_branch("main"),
        )
        .unwrap();

        let value = harness.finished(id).value.unwrap();
        assert_eq!(value["summary"]["count"], 4);
        assert_eq!(value["summary"]["collapsible"], true);
        assert_eq!(value["summary"]["viewId"], value["viewId"]);
        assert_ne!(value["viewId"], FIRST_VIEW);
    }

    #[test]
    fn the_new_view_replaces_that_of_its_tab() {
        let repo = repo_with_merge();
        let harness = Harness::new(&repo, "exit 1");
        let id = start_view(
            &harness.log,
            &harness.runner,
            repo.path(),
            7,
            by_branch("main"),
        )
        .unwrap();
        let first = harness.finished(id).value.unwrap()["viewId"]
            .as_u64()
            .unwrap() as u32;

        let id = start_view(
            &harness.log,
            &harness.runner,
            repo.path(),
            7,
            by_branch("main"),
        )
        .unwrap();
        let second = harness.finished(id).value.unwrap()["viewId"]
            .as_u64()
            .unwrap() as u32;

        let log = harness.log.lock().unwrap();
        let open = log.as_ref().unwrap();
        assert!(rows_in(open, first, 0, 10).is_empty());
        assert_eq!(rows_in(open, second, 0, 10).len(), 4);
        // The one from another tab is still there.
        assert_eq!(rows_in(open, FIRST_VIEW, 0, 10).len(), 4);
    }

    #[test]
    fn opening_a_view_does_not_hold_the_log_while_git_runs() {
        let repo = repo_with_merge();
        let files = TempDir::new().unwrap();
        let started = files.path().join("started");
        let release = files.path().join("release");
        let harness = Harness::new(
            &repo,
            &format!(
                "touch '{}'\nwhile [ ! -e '{}' ]; do sleep 0.01; done",
                started.display(),
                release.display()
            ),
        );

        let id = start_view(&harness.log, &harness.runner, repo.path(), 0, by_text("x")).unwrap();
        wait_for(&started);

        {
            let log = harness.log.try_lock().unwrap();
            assert_eq!(rows_in(log.as_ref().unwrap(), FIRST_VIEW, 0, 10).len(), 4);
        }
        fs::write(&release, "").unwrap();
        assert_eq!(harness.finished(id).state, TaskState::Done);
    }

    #[test]
    fn opening_another_view_cancels_the_previous_one_of_that_tab() {
        let repo = repo_with_merge();
        let files = TempDir::new().unwrap();
        let started = files.path().join("started");
        let harness = Harness::new(
            &repo,
            &format!("touch '{}'\nexec sleep 30", started.display()),
        );
        let slow = start_view(&harness.log, &harness.runner, repo.path(), 3, by_text("x")).unwrap();
        wait_for(&started);

        let other_tab =
            start_view(&harness.log, &harness.runner, repo.path(), 4, by_text("x")).unwrap();
        let next = start_view(
            &harness.log,
            &harness.runner,
            repo.path(),
            3,
            by_branch("main"),
        )
        .unwrap();

        assert_eq!(harness.finished(slow).state, TaskState::Cancelled);
        assert_eq!(harness.finished(next).state, TaskState::Done);
        harness.runner.cancel(other_tab);
    }

    #[test]
    fn view_of_a_log_that_is_gone_is_discarded() {
        let repo = repo_with_merge();
        let files = TempDir::new().unwrap();
        let started = files.path().join("started");
        let release = files.path().join("release");
        let harness = Harness::new(
            &repo,
            &format!(
                "touch '{}'\nwhile [ ! -e '{}' ]; do sleep 0.01; done",
                started.display(),
                release.display()
            ),
        );
        let id = start_view(&harness.log, &harness.runner, repo.path(), 0, by_text("x")).unwrap();
        wait_for(&started);

        *harness.log.lock().unwrap() = Some(open_repo(repo.path()).unwrap());
        fs::write(&release, "").unwrap();

        assert_eq!(harness.finished(id).state, TaskState::Cancelled);
    }

    #[test]
    fn without_an_open_log_there_is_no_view() {
        let repo = repo_with_merge();
        let harness = Harness::new(&repo, "exit 1");
        *harness.log.lock().unwrap() = None;

        let error = start_view(
            &harness.log,
            &harness.runner,
            repo.path(),
            0,
            ViewSpec::default(),
        )
        .err()
        .unwrap();

        assert_eq!(error.message, "The log is not open");
    }

    #[test]
    fn the_specification_comes_from_the_frontend_in_camel_case() {
        let spec: ViewSpec = serde_json::from_str(
            r#"{"sort":"standard","longEdges":true,"filter":{
                "branches":["main"],"users":["*"],"since":5,"until":null,"paths":["a"],
                "text":{"text":"x","regex":true,"matchCase":true},"hashes":[],
                "revisions":["c"],"ranges":[["a","b"]]}}"#,
        )
        .unwrap();

        assert_eq!(spec.sort, SortType::Standard);
        assert!(spec.long_edges);
        assert_eq!(spec.filter.since, Some(5));
        assert!(spec.filter.text.unwrap().match_case);
        assert_eq!(spec.filter.revisions, ["c"]);
        assert_eq!(spec.filter.ranges, [("a".to_string(), "b".to_string())]);
        assert_eq!(
            serde_json::from_str::<ViewSpec>("{}").unwrap(),
            ViewSpec::default()
        );
    }

    #[test]
    fn row_carries_its_highlights() {
        let repo = repo_with_merge();
        repo.git(&[
            "-c",
            "user.name=Another",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "foreign",
        ]);
        let open = open_repo(repo.path()).unwrap();

        let rows = rows(&open, 0, 10);

        assert_eq!(rows[0].subject, "foreign");
        assert!(!rows[0].highlight.bold);
        assert!(rows[1].highlight.muted && rows[1].highlight.bold);
        assert!(rows[1].highlight.branch);
    }

    #[test]
    fn log_users_without_repeating() {
        let repo = repo_with_merge();
        repo.git(&[
            "-c",
            "user.name=Another",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "foreign",
        ]);
        let open = open_repo(repo.path()).unwrap();

        let mut names: Vec<String> = users_of(&open)
            .iter()
            .map(|person| person.name.to_string())
            .collect();
        names.sort();

        assert_eq!(names, ["Another", "Test"]);
    }

    #[test]
    fn log_references() {
        let repo = repo_with_merge();
        repo.git(&["tag", "v1"]);
        let open = open_repo(repo.path()).unwrap();

        let refs: Vec<_> = refs_of(&open)
            .into_iter()
            .map(|git_ref| (git_ref.name, git_ref.kind))
            .collect();

        assert_eq!(
            refs,
            [
                ("HEAD".to_string(), RefKind::Head),
                ("main".to_string(), RefKind::Local),
                ("v1".to_string(), RefKind::Tag),
            ]
        );
    }

    fn rows(open: &OpenRepo, start: u32, count: u32) -> Vec<LogRow> {
        rows_in(open, summary(open).view_id, start, count)
    }

    fn shared(open: OpenRepo) -> SharedLog {
        Arc::new(Mutex::new(Some(open)))
    }

    fn shared_rows(log: &SharedLog, start: u32, count: u32) -> Vec<LogRow> {
        rows(log.lock().unwrap().as_ref().unwrap(), start, count)
    }

    #[test]
    fn log_rows_out_of_range() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        let open = open_repo(repo.path()).unwrap();
        assert!(rows(&open, 100, 10).is_empty());
    }

    #[test]
    fn log_rows_of_unknown_view() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        let open = open_repo(repo.path()).unwrap();
        assert!(rows_in(&open, 99, 0, 10).is_empty());
    }

    #[test]
    fn log_rows_returns_graph_and_data() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        repo.commit("a.txt", b"2", "two");
        let open = open_repo(repo.path()).unwrap();

        let rows = rows(&open, 0, 10);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].subject, "two");
        assert_eq!(rows[0].author, "Test");
        assert_eq!(rows[0].print.node_x, 0);
        assert_eq!(rows[0].print.edges.len(), 1);
        assert_eq!(rows[1].print.edges.len(), 1);
        assert_eq!(summary(&open).count, 2);
    }

    #[test]
    fn log_rows_carries_the_references_with_their_kind() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        repo.git(&["tag", "v1"]);

        let open = open_repo(repo.path()).unwrap();

        let refs: Vec<_> = rows(&open, 0, 1)[0]
            .refs
            .iter()
            .map(|git_ref| (git_ref.name.clone(), git_ref.kind))
            .collect();
        assert_eq!(
            refs,
            [
                ("HEAD".to_string(), RefKind::Head),
                ("main".to_string(), RefKind::Local),
                ("v1".to_string(), RefKind::Tag),
            ]
        );
    }

    #[test]
    fn row_arrives_at_the_frontend_in_camel_case() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        repo.commit("a.txt", b"2", "two");
        let open = open_repo(repo.path()).unwrap();

        let row = serde_json::to_value(&rows(&open, 0, 1)[0]).unwrap();

        assert_eq!(row["print"]["nodeX"], 0);
        assert_eq!(row["print"]["edges"][0]["otherX"], 0);
        assert_eq!(row["print"]["edges"][0]["style"], "usual");
        assert_eq!(row["refs"][0]["kind"], "head");
        assert_eq!(row["commitTime"], row["time"]);
        assert_eq!(row["samePerson"], true);
        assert_eq!(row["parentCount"], 1);
    }

    #[test]
    fn row_counts_the_parents_of_a_merge() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        repo.git(&["checkout", "-q", "-b", "side"]);
        repo.commit("b.txt", b"b", "side");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("c.txt", b"c", "main");
        repo.git(&["merge", "-q", "--no-ff", "-m", "join", "side"]);
        let open = open_repo(repo.path()).unwrap();

        let all = rows(&open, 0, 10);

        let counts: Vec<u32> = all.iter().map(|row| row.parent_count).collect();
        assert_eq!(counts, [2, 1, 1, 0]);
    }

    #[test]
    fn row_carries_the_two_forms_of_its_labels() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        repo.git(&["branch", "side"]);
        repo.git(&["tag", "v1"]);
        let open = open_repo(repo.path()).unwrap();

        let labels = rows(&open, 0, 1).remove(0).labels;

        assert_eq!(
            labels.compact,
            [LabelGroup {
                name: "main".to_string(),
                kinds: vec![RefKind::Head, RefKind::Local, RefKind::Local, RefKind::Tag],
            }]
        );
        let normal: Vec<_> = labels.normal.iter().map(|group| &*group.name).collect();
        assert_eq!(normal, ["main", "side", "v1"]);
    }

    #[test]
    fn row_says_if_the_committer_is_someone_else() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        repo.git(&[
            "-c",
            "user.name=Another",
            "-c",
            "user.email=another@example.com",
            "commit",
            "-q",
            "--amend",
            "--no-edit",
        ]);
        let open = open_repo(repo.path()).unwrap();

        assert!(!rows(&open, 0, 1)[0].same_person);
    }

    #[test]
    fn repository_error() {
        let dir = TempDir::new().unwrap();
        let error = open_repo(dir.path()).err().unwrap();
        assert_eq!(error.kind, "not_a_repository");
        assert!(error.message.contains(&dir.path().display().to_string()));
    }
}
