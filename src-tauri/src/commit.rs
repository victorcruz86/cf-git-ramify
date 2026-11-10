use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::branches::{self, Repo};
use crate::commands::ApiError;
use crate::diff;
use crate::git::{self, GitError};
use crate::hunks;
use crate::settings::Settings;
use crate::snapshots;
use crate::storage;

pub const JOURNAL_FILE: &str = "commit-journal.json";
pub const LOCK_FILE: &str = "commit.lock";

const COMMIT_REASON: &str = "before commit";
const RECOVERY_OUTCOME: &str = "The stage of an interrupted commit was being recovered.";
const DEFAULT_MODE: &str = "100644";
const EXECUTABLE_MODE: &str = "100755";
const COMMIT_HOOKS: [&str; 2] = ["hooks/pre-commit", "hooks/commit-msg"];
// The message is written in UTF-8; another encoding in the configuration would record it wrongly.
const MESSAGE_ENCODING: &str = "i18n.commitEncoding=UTF-8";
const NOTHING_TO_COMMIT: [&str; 3] = ["no changes", "nothing added to commit", "nothing to commit"];
const REBASE_DIRS: [&str; 2] = ["rebase-apply", "rebase-merge"];
const REBASE_INTERACTIVE: &str = "interactive";
const MERGE_HEAD: &str = "MERGE_HEAD";
const MERGE_MSG: &str = "MERGE_MSG";
const SQUASH_MSG: &str = "SQUASH_MSG";
const NO_MERGE: &str = "No merge in progress";
pub(crate) const UNMERGED_PATHS: &str =
    "Committing is not possible because you have unmerged files.";
const RECOMMENDED_AUTOCRLF: &str = "input";

#[derive(Debug, Clone, Deserialize)]
pub struct PartialFile {
    pub path: String,
    // The bytes of the blob, in the form git stores (the one `git add` would give for the file), not those
    // on disk: they are written as they are, without `autocrlf`, `eol` or filters.
    pub content: Vec<u8>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitOptions {
    pub amend: bool,
    pub sign_off: bool,
    pub skip_hooks: bool,
    pub author: Option<String>,
    pub cleanup: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CommitRequest {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub partial: Vec<PartialFile>,
    pub message: String,
    pub options: CommitOptions,
}

// What happened to an interrupted commit. `skipped` carries the path with its mode and its hash; `pending`
// says the commit is still alive; `error`, that it could not be finished (index locked, with the
// journal kept) or that the journal could not be deleted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryReport {
    pub restored: Vec<String>,
    pub skipped: Vec<String>,
    pub pending: bool,
    pub error: Option<String>,
}

impl RecoveryReport {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum CommitError {
    Unmerged,
    // A commit with only part of the merge would have two parents without containing the merge.
    PartialMerge,
    NothingToCommit,
    Gpg(String),
    Hook(String),
    Git(GitError),
    OutsideRepository(String),
    Snapshot(String),
    Journal(String),
    // It is not known what was in the stage: it is the only thing that prevents opening the repository.
    UnreadableJournal(String),
    // The index was busy and remains as it was.
    IndexBusy,
    // A `git commit` is still alive, perhaps from a session that died, and is still going to read the index.
    CommitRunning,
    // Paths that an interrupted commit could not return to the stage, with their mode and hash.
    RecoverySkipped(Vec<String>),
    // The index did not end up as it was. `detail` says what corresponds to each path, and the blobs
    // are still in the object database; `outcome` says what happened to the commit.
    Restore {
        paths: Vec<String>,
        detail: String,
        outcome: String,
    },
}

impl fmt::Display for CommitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unmerged => write!(f, "Cannot commit with unresolved files"),
            Self::PartialMerge => write!(
                f,
                "A merge must be committed whole: include all of its changes in the commit, without \
                 leaving files out or choosing lines"
            ),
            Self::NothingToCommit => write!(f, "Nothing to commit"),
            Self::Gpg(output) => write!(f, "Could not sign the commit: {output}"),
            Self::Hook(output) => write!(f, "A hook rejected the commit: {output}"),
            Self::Git(error) => write!(f, "{}", git_message(error)),
            Self::Snapshot(error) => write!(
                f,
                "Could not save the backup copy and nothing was touched: {error}"
            ),
            Self::Journal(error) | Self::UnreadableJournal(error) => write!(f, "{error}"),
            Self::IndexBusy => write!(
                f,
                "Another Git process is using the index and nothing was touched. Wait for it to \
                 finish and try again"
            ),
            Self::CommitRunning => write!(
                f,
                "A commit is still running in this repository. Wait for it to finish and try \
                 again"
            ),
            Self::RecoverySkipped(entries) => write!(
                f,
                "An interrupted commit has not returned these paths to the stage, because they changed afterwards \
                 or Git could not restore them: {}. Nothing else was done: try again",
                entries.join("; ")
            ),
            Self::OutsideRepository(path) => {
                write!(f, "The path \"{path}\" is not inside the repository")
            }
            Self::Restore {
                paths,
                detail,
                outcome,
            } => write!(
                f,
                "{outcome} Could not leave the stage as it was in: {}. {detail}",
                paths.join(", ")
            ),
        }
    }
}

impl std::error::Error for CommitError {}

impl From<GitError> for CommitError {
    fn from(error: GitError) -> Self {
        Self::Git(error)
    }
}

fn git_message(error: &GitError) -> String {
    ApiError::from(error.clone()).message
}

fn check_paths(repo: &Path, request: &CommitRequest) -> Result<(), CommitError> {
    let partial = request.partial.iter().map(|file| &file.path);
    for path in request.added.iter().chain(&request.removed).chain(partial) {
        snapshots::check_path(repo, path).map_err(|error| match error.kind() {
            io::ErrorKind::InvalidInput => CommitError::OutsideRepository(path.clone()),
            _ => CommitError::Git(GitError::Io(error.to_string())),
        })?;
    }
    Ok(())
}

pub fn has_unmerged(repo: &Path) -> Result<bool, GitError> {
    let output = git::run(repo, &["diff", "--name-only", "--diff-filter=U", "-z"])?;
    Ok(!output.is_empty())
}

// A path that is not understood could not be taken out of the stage and would enter the commit.
fn path_from(bytes: &[u8]) -> Result<String, GitError> {
    String::from_utf8(bytes.to_vec())
        .map_err(|_| GitError::Io("The stage contains a path that is not UTF-8".to_string()))
}

fn records(output: &[u8]) -> impl Iterator<Item = &[u8]> {
    output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
}

// Records `status\0path\0`. Without renames, one is a deletion and an addition separately.
fn staged_paths(repo: &Path) -> Result<BTreeSet<String>, GitError> {
    let args = ["diff", "--cached", "--name-status", "-z", "--no-renames"];
    let output = git::run(repo, &args)?;
    records(&output).skip(1).step_by(2).map(path_from).collect()
}

fn selected_paths(request: &CommitRequest) -> BTreeSet<String> {
    let partial = request.partial.iter().map(|file| &file.path);
    request
        .added
        .iter()
        .chain(&request.removed)
        .chain(partial)
        .cloned()
        .collect()
}

// What the index had for a path. The blobs are still in the object database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Saved {
    Absent,
    // `git add -N`: the entry has no content and `--cacheinfo` would leave it as an empty file.
    IntentToAdd,
    Entry { mode: String, hash: String },
}

// A record `fields separated by spaces<TAB>path`, with the mode first.
fn parse_entry(record: &[u8], hash_field: usize) -> Result<Option<(String, Saved)>, GitError> {
    let Some(tab) = record.iter().position(|byte| *byte == b'\t') else {
        return Ok(None);
    };
    let header = String::from_utf8_lossy(&record[..tab]);
    let fields: Vec<&str> = header.split(' ').collect();
    let (Some(mode), Some(hash)) = (fields.first(), fields.get(hash_field)) else {
        return Ok(None);
    };
    let entry = Saved::Entry {
        mode: mode.to_string(),
        hash: hash.to_string(),
    };
    Ok(Some((path_from(&record[tab + 1..])?, entry)))
}

fn absent_entries(paths: &[String]) -> BTreeMap<String, Saved> {
    paths
        .iter()
        .map(|path| (path.clone(), Saved::Absent))
        .collect()
}

// Entries `mode hash stage<TAB>path`. Only stage 0: conflicts have already been rejected.
pub(crate) fn read_entries(
    repo: &Path,
    paths: &[String],
) -> Result<BTreeMap<String, Saved>, GitError> {
    let mut saved = absent_entries(paths);
    let output = git::git_with_paths(repo, false, &git::exact(&["ls-files", "-s", "-z"]), paths)?;
    for record in records(&output) {
        if let Some((path, entry)) = parse_entry(record, 1)? {
            if let Some(slot) = saved.get_mut(&path) {
                *slot = entry;
            }
        }
    }
    // An entry that the disk sees as new relative to the index can only be from `add -N`.
    let added = [
        "diff",
        "--name-only",
        "--no-renames",
        "--diff-filter=A",
        "-z",
    ];
    let output = git::git_with_paths(repo, false, &git::exact(&added), paths)?;
    for path in records(&output) {
        if let Some(slot) = saved.get_mut(&path_from(path)?) {
            *slot = Saved::IntentToAdd;
        }
    }
    Ok(saved)
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

// Without an index entry (new file, or renamed and not yet staged) the mode is on disk.
fn partial_mode<'a>(repo: &Path, path: &str, saved: &'a BTreeMap<String, Saved>) -> &'a str {
    match saved.get(path) {
        Some(Saved::Entry { mode, .. }) => mode,
        _ if is_executable(&repo.join(path)) => EXECUTABLE_MODE,
        _ => DEFAULT_MODE,
    }
}

pub(crate) fn write_entry(repo: &Path, file: &PartialFile, mode: &str) -> Result<(), GitError> {
    let hash_args = ["hash-object", "-w", "--stdin", "--no-filters"];
    let hash = git::run_with_input(repo, &hash_args, &file.content)?;
    let hash = String::from_utf8_lossy(&hash);
    let info = format!("{mode},{},{}", hash.trim(), file.path);
    git::run_mutating(
        repo,
        &git::exact(&["update-index", "--add", "--cacheinfo", &info]),
    )
    .map(drop)
}

pub(crate) fn write_partial(
    repo: &Path,
    files: &[PartialFile],
    saved: &BTreeMap<String, Saved>,
) -> Result<(), GitError> {
    for file in files {
        write_entry(repo, file, partial_mode(repo, &file.path, saved))?;
    }
    Ok(())
}

// Without commits there is no HEAD to go back to. `-f`: without it, `rm --cached` refuses if the stage differs
// from the disk and from HEAD, and on a branch without commits everything in the stage differs from HEAD.
pub(crate) fn unstage(repo: &Path, head_exists: bool, paths: &[String]) -> Result<(), GitError> {
    let head: &[&str] = if head_exists {
        &["reset", "HEAD"]
    } else {
        &["rm", "--cached", "--quiet", "-f"]
    };
    git::git_with_paths(repo, true, &git::exact(head), paths).map(drop)
}

// Like IntelliJ, it removes first and then adds: on a case-insensitive volume, `add` of `A.txt`
// with `a.txt` still in the index would keep the old name. `-f` on `add` allows committing an
// ignored file that was selected; the same `-f` of `rm` as in `unstage`. What is versioned
// is looked up with the bytes of its entry, and what is new with the name `add` reads from disk.
pub(crate) fn stage_selection(
    repo: &Path,
    request: &CommitRequest,
    saved: &BTreeMap<String, Saved>,
) -> Result<(), GitError> {
    let partial: BTreeSet<&String> = request.partial.iter().map(|file| &file.path).collect();
    let added: BTreeSet<&String> = request.added.iter().collect();
    let (tracked, untracked): (Vec<String>, Vec<String>) = request
        .added
        .iter()
        .filter(|path| !partial.contains(path))
        .cloned()
        .partition(|path| saved[path] != Saved::Absent);
    let to_remove: Vec<String> = request
        .removed
        .iter()
        .filter(|path| !partial.contains(path) && !added.contains(path))
        .cloned()
        .collect();
    let rm = ["rm", "--cached", "--quiet", "-f", "--ignore-unmatch"];
    git::git_with_paths(repo, true, &git::exact(&rm), &to_remove)?;
    let add = ["add", "--ignore-errors", "-f"];
    git::git_with_paths(repo, true, &git::exact(&add), &tracked)?;
    git::git_with_paths(repo, true, &add, &untracked)?;
    Ok(())
}

// `git commit -F` needs a real file. It is deleted when dropped, also if the commit fails.
fn message_file(message: &str) -> io::Result<NamedTempFile> {
    let mut file = tempfile::Builder::new().prefix("ramify-commit-").tempfile()?;
    file.write_all(message.as_bytes())?;
    file.flush()?;
    Ok(file)
}

// `GitRepositoryReader.readHooksInfo`: an executable `pre-commit` or `commit-msg` where git looks for
// them (`core.hooksPath`, worktrees). It is also what offers `Run Git hooks`.
pub fn has_commit_hooks(repo: &Path) -> bool {
    COMMIT_HOOKS
        .iter()
        .any(|hook| git::git_path(repo, hook).is_ok_and(|path| is_executable(&path)))
}

// Git's notice goes through stdout; the hooks' output, through stderr.
fn reports_nothing(stdout: &str) -> bool {
    stdout.lines().any(|line| {
        let line = line.to_lowercase();
        NOTHING_TO_COMMIT.iter().any(|text| line.starts_with(text))
    })
}

fn classify(repo: &Path, options: &CommitOptions, error: GitError) -> CommitError {
    let GitError::Failed { stdout, stderr, .. } = &error else {
        return error.into();
    };
    if stderr.contains("gpg failed to sign") {
        return CommitError::Gpg(stderr.trim().to_string());
    }
    if reports_nothing(stdout) {
        return CommitError::NothingToCommit;
    }
    if !options.skip_hooks && has_commit_hooks(repo) {
        return CommitError::Hook(
            format!("{}\n{}", stdout.trim(), stderr.trim())
                .trim()
                .to_string(),
        );
    }
    error.into()
}

fn commit_args<'a>(
    message_file: &'a str,
    options: &CommitOptions,
    author: &'a Option<String>,
) -> Vec<&'a str> {
    let mut args = vec!["-c", MESSAGE_ENCODING, "commit", "-F", message_file];
    if options.amend {
        args.push("--amend");
    }
    if options.sign_off {
        args.push("--signoff");
    }
    if options.skip_hooks {
        args.push("--no-verify");
    }
    if options.cleanup {
        args.push("--cleanup=strip");
    }
    args.extend(author.as_deref());
    args.push("--");
    args
}

fn run_commit(repo: &Path, request: &CommitRequest, lock: &CommitLock) -> Result<(), CommitError> {
    let file = message_file(&request.message).map_err(|error| GitError::Io(error.to_string()))?;
    let path = file
        .path()
        .to_str()
        .ok_or_else(|| GitError::Io("Invalid temporary path".to_string()))?;
    let options = &request.options;
    let author = options
        .author
        .as_ref()
        .map(|author| format!("--author={author}"));
    let args = commit_args(path, options, &author);
    git::run_mutating_holding(repo, &args, &lock.0)
        .map(drop)
        .map_err(|error| classify(repo, options, error))
}

fn write_commit(
    repo: &Path,
    request: &CommitRequest,
    plan: &Plan,
    lock: &CommitLock,
) -> Result<(), CommitError> {
    write_partial(repo, &request.partial, &plan.saved)?;
    unstage(repo, plan.head_exists, &plan.excluded)?;
    stage_selection(repo, request, &plan.saved)?;
    run_commit(repo, request, lock)
}

struct RestoreFailure {
    paths: Vec<String>,
    detail: String,
    index_locked: bool,
}

impl RestoreFailure {
    fn add(&mut self, paths: &[String], error: GitError) {
        self.paths.extend_from_slice(paths);
        self.index_locked |= error == GitError::IndexLocked;
        self.detail.push_str(&git_message(&error));
        self.detail.push_str(". ");
    }

    // With the index locked we do not continue: each attempt would wait for the lock only to fail the same way.
    fn attempt(&mut self, paths: &[String], run: impl FnOnce() -> Result<(), GitError>) {
        if self.index_locked {
            self.paths.extend_from_slice(paths);
        } else if let Err(error) = run() {
            self.add(paths, error);
        }
    }
}

fn cache_info(path: &str, saved: &BTreeMap<String, Saved>) -> String {
    match &saved[path] {
        Saved::Entry { mode, hash } => format!("{mode},{hash},{path}"),
        _ => unreachable!("only paths with an entry have cacheinfo"),
    }
}

fn expected_entry(path: &str, saved: &Saved) -> String {
    match saved {
        Saved::Entry { mode, hash } => format!("\"{path}\" with {mode} {hash}"),
        Saved::IntentToAdd => format!("\"{path}\" with git add -N"),
        Saved::Absent => format!("\"{path}\" not in the stage"),
    }
}

// With exact names an NFD entry matches. An NFC entry with the NFD name on disk does not match that way,
// and `add` exits with 0 without adding anything, but it does match with git's conversion. An NFD one
// would be converted wrongly, hence the order and checking the result.
fn add_intent(repo: &Path, path: &str) -> Result<(), GitError> {
    let add = ["add", "--intent-to-add"];
    let paths = [path.to_string()];
    for args in [git::exact(&add), add.to_vec()] {
        git::git_with_paths(repo, true, &args, &paths)?;
        if read_entries(repo, &paths)?[path] == Saved::IntentToAdd {
            return Ok(());
        }
    }
    Err(GitError::Io(format!(
        "Could not mark \"{path}\" with git add -N"
    )))
}

// Each `update-index` batch is applied whole or not at all, so the ones that fail say
// exactly which paths ended up wrong, and the others are restored anyway. Removal comes first, so
// that a file and a folder with the same name do not clash in the index.
fn restore(
    repo: &Path,
    saved: &BTreeMap<String, Saved>,
    paths: &[String],
) -> Result<(), RestoreFailure> {
    let mut failure = RestoreFailure {
        paths: Vec::new(),
        detail: String::new(),
        index_locked: false,
    };
    let (entries, others): (Vec<String>, Vec<String>) = paths
        .iter()
        .cloned()
        .partition(|path| matches!(saved[path], Saved::Entry { .. }));
    let intents: Vec<String> = others
        .iter()
        .filter(|path| saved[*path] == Saved::IntentToAdd)
        .cloned()
        .collect();

    for chunk in others.chunks(git::PATHS_PER_COMMAND) {
        let args = git::exact(&["update-index", "--force-remove"]);
        failure.attempt(chunk, || {
            git::git_with_paths(repo, true, &args, chunk).map(drop)
        });
    }
    for chunk in entries.chunks(git::PATHS_PER_COMMAND) {
        let infos: Vec<String> = chunk.iter().map(|path| cache_info(path, saved)).collect();
        let mut args = git::exact(&["update-index", "--add"]);
        for info in &infos {
            args.extend(["--cacheinfo", info]);
        }
        failure.attempt(chunk, || git::run_mutating(repo, &args).map(drop));
    }
    for path in intents {
        failure.attempt(std::slice::from_ref(&path), || add_intent(repo, &path));
    }

    if failure.paths.is_empty() {
        return Ok(());
    }
    failure.paths.sort();
    failure.paths.dedup();
    let expected: Vec<String> = failure
        .paths
        .iter()
        .map(|path| expected_entry(path, &saved[path]))
        .collect();
    failure.detail = format!("{}It should be: {}", failure.detail, expected.join("; "));
    Err(failure)
}

fn head_hash(repo: &Path) -> Result<String, GitError> {
    let output = git::run(repo, &["rev-parse", "HEAD"])?;
    Ok(String::from_utf8_lossy(&output).trim().to_string())
}

fn outcome_of(repo: &Path, result: &Result<(), CommitError>) -> String {
    match result {
        Ok(()) => match head_hash(repo) {
            Ok(hash) => format!("The commit {hash} was created."),
            Err(_) => "The commit was created.".to_string(),
        },
        Err(error) => format!("The commit was not created: {error}."),
    }
}

// What needs to be known before touching anything.
struct Plan {
    head_exists: bool,
    excluded: Vec<String>,
    affected: Vec<String>,
    saved: BTreeMap<String, Saved>,
    // What `unstage` leaves in the index for each excluded path.
    left: BTreeMap<String, Saved>,
}

// `unstage` leaves the HEAD entry, or none. It is computed before doing it: the journal must
// be written before touching anything.
pub(crate) fn left_entries(
    repo: &Path,
    head_exists: bool,
    paths: &[String],
) -> Result<BTreeMap<String, Saved>, GitError> {
    let mut left = absent_entries(paths);
    if !head_exists {
        return Ok(left);
    }
    let head = git::exact(&["ls-tree", "-r", "-z", "HEAD"]);
    let output = git::git_with_paths(repo, false, &head, paths)?;
    for record in records(&output) {
        if let Some((path, entry)) = parse_entry(record, 2)? {
            if let Some(slot) = left.get_mut(&path) {
                *slot = entry;
            }
        }
    }
    Ok(left)
}

fn plan(repo: &Path, request: &CommitRequest) -> Result<Plan, CommitError> {
    if has_unmerged(repo)? {
        return Err(CommitError::Unmerged);
    }
    let head_exists = git::head_exists(repo)?;
    let selected = selected_paths(request);
    let excluded: Vec<String> = staged_paths(repo)?.difference(&selected).cloned().collect();
    let partial = !excluded.is_empty() || !request.partial.is_empty();
    if partial && git::git_path(repo, MERGE_HEAD)?.exists() {
        return Err(CommitError::PartialMerge);
    }
    let affected: Vec<String> = selected.iter().chain(&excluded).cloned().collect();
    let saved = read_entries(repo, &affected)?;
    let left = left_entries(repo, head_exists, &excluded)?;
    Ok(Plan {
        head_exists,
        excluded,
        affected,
        saved,
        left,
    })
}

// What was chosen replaces what was in the stage, as in IntelliJ. If that content is neither in
// HEAD nor on disk, it would only remain in a loose blob: a copy is saved before continuing.
fn copy_replaced_stage(
    repo: &Path,
    dir: &Path,
    request: &CommitRequest,
) -> Result<(), CommitError> {
    let partial = request.partial.iter().map(|file| &file.path);
    let paths: Vec<String> = request.added.iter().chain(partial).cloned().collect();
    let at_risk: Vec<String> = snapshots::staged_only(repo, &paths)?.into_iter().collect();
    if at_risk.is_empty() {
        return Ok(());
    }
    snapshots::create(dir, repo, &at_risk, COMMIT_REASON)
        .map_err(|error| CommitError::Snapshot(error.to_string()))?;
    Ok(())
}

// What must be returned to an excluded path, and what must remain in the index to do so.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct JournalEntry {
    restore: Saved,
    left: Saved,
}

fn lock_error(error: io::Error) -> CommitError {
    CommitError::Journal(format!(
        "Could not check whether a commit is running: {error}"
    ))
}

// It is released by hand: closing the file is not enough, because a process that another thread is launching at
// that moment holds a copy of the description until its `exec`, and the `flock` would remain taken.
struct CommitLock(fs::File);

impl Drop for CommitLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

fn try_lock(file: fs::File) -> Result<Option<CommitLock>, CommitError> {
    match file.try_lock() {
        Ok(()) => Ok(Some(CommitLock(file))),
        Err(fs::TryLockError::WouldBlock) => Ok(None),
        Err(fs::TryLockError::Error(error)) => Err(lock_error(error)),
    }
}

// While the commit lasts, and after the app dies if `git commit` is still alive: the `flock` belongs
// to the file description that `git commit` inherits. The file is never deleted.
fn commit_lock(dir: &Path) -> Result<CommitLock, CommitError> {
    let file = fs::create_dir_all(dir)
        .and_then(|()| {
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(dir.join(LOCK_FILE))
        })
        .map_err(lock_error)?;
    try_lock(file)?.ok_or(CommitError::CommitRunning)
}

// Only what was excluded: if the commit did get created, restoring it is still correct, and what was
// selected is already in HEAD and must not go back.
fn write_journal(dir: &Path, plan: &Plan) -> Result<(), CommitError> {
    if plan.excluded.is_empty() {
        return Ok(());
    }
    let entries: BTreeMap<&String, JournalEntry> = plan
        .excluded
        .iter()
        .map(|path| {
            let entry = JournalEntry {
                restore: plan.saved[path].clone(),
                left: plan.left[path].clone(),
            };
            (path, entry)
        })
        .collect();
    let json = serde_json::to_vec_pretty(&entries).map_err(io::Error::other);
    json.and_then(|json| {
        fs::create_dir_all(dir)?;
        storage::write_atomic(&dir.join(JOURNAL_FILE), &json)
    })
    .map_err(|error| {
        CommitError::Journal(format!(
            "Could not save the stage journal and nothing was touched: {error}"
        ))
    })
}

fn clear_journal(dir: &Path) -> io::Result<()> {
    match fs::remove_file(dir.join(JOURNAL_FILE)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

fn read_journal(dir: &Path) -> Result<Option<BTreeMap<String, JournalEntry>>, CommitError> {
    let unreadable = |error: String| {
        CommitError::UnreadableJournal(format!(
            "Could not read the journal of an interrupted commit, so the stage may be \
             incomplete. It is kept at {}: {error}",
            dir.join(JOURNAL_FILE).display()
        ))
    };
    let bytes = match fs::read(dir.join(JOURNAL_FILE)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(unreadable(error.to_string())),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| unreadable(error.to_string()))
}

fn index_as_planned(repo: &Path, plan: &Plan) -> bool {
    read_entries(repo, &plan.affected).is_ok_and(|entries| entries == plan.saved)
}

// Follows the order of IntelliJ's `commitUsingIndex`. Before touching anything it saves what the
// index has for each affected path: if the commit succeeds what was excluded comes back and, if it fails, everything.
// The journal covers the case of the app dying midway, and is only deleted after restoring.
pub fn commit(repo: &Path, dir: &Path, request: &CommitRequest) -> Result<String, CommitError> {
    check_paths(repo, request)?;
    let lock = commit_lock(dir)?;
    check_recovery(&apply_journal(repo, dir)?)?;
    let plan = plan(repo, request)?;
    copy_replaced_stage(repo, dir, request)?;
    write_journal(dir, &plan)?;

    let result = write_commit(repo, request, &plan, &lock);

    // Looking at the index also accounts for the batches: from a step that fails, the first one may have gone in.
    if result == Err(GitError::IndexLocked.into()) && index_as_planned(repo, &plan) {
        return Err(CommitError::IndexBusy);
    }
    let to_restore = if result.is_ok() {
        &plan.excluded
    } else {
        &plan.affected
    };
    if let Err(RestoreFailure { paths, detail, .. }) = restore(repo, &plan.saved, to_restore) {
        return Err(CommitError::Restore {
            paths,
            detail,
            outcome: outcome_of(repo, &result),
        });
    }
    clear_journal(dir).map_err(|error| {
        let outcome = outcome_of(repo, &result);
        CommitError::Journal(format!(
            "{outcome} The stage is fine, but its journal could not be deleted: {error}"
        ))
    })?;
    result?;
    Ok(head_hash(repo)?)
}

// The commit of staging area mode: the index as it is. It takes nothing out of the stage, so there is
// nothing to note in the journal nor to give back afterwards.
pub fn commit_staged(
    repo: &Path,
    dir: &Path,
    options: &CommitOptions,
    message: &str,
) -> Result<String, CommitError> {
    let lock = commit_lock(dir)?;
    check_recovery(&apply_journal(repo, dir)?)?;
    if has_unmerged(repo)? {
        return Err(CommitError::Unmerged);
    }
    let request = CommitRequest {
        message: message.to_string(),
        options: options.clone(),
        ..Default::default()
    };
    run_commit(repo, &request, &lock)?;
    Ok(head_hash(repo)?)
}

// The unresolved ones that concern `paths`: themselves, what hangs below a requested folder and the
// unresolved files that are ancestors of a requested path (`git add d/f` would remove the unresolved
// file `d` from the index). All the unresolved ones are read, which is few, and compared by
// components, like a literal pathspec.
pub(crate) fn conflicted(repo: &Path, paths: &[String]) -> Result<BTreeSet<String>, GitError> {
    if paths.is_empty() {
        return Ok(BTreeSet::new());
    }
    let output = git::run(repo, &git::exact(&["ls-files", "-u", "-z"]))?;
    let asked: BTreeSet<&str> = paths
        .iter()
        .map(|path| path.trim_end_matches('/'))
        .collect();
    let everything = asked.iter().any(|path| path.is_empty() || *path == ".");
    let mut names = BTreeSet::new();
    for record in records(&output) {
        if let Some((path, _)) = parse_entry(record, 1)? {
            if everything || touches(&asked, &path) {
                names.insert(path);
            }
        }
    }
    Ok(names)
}

fn touches(asked: &BTreeSet<&str>, unmerged: &str) -> bool {
    if asked.contains(unmerged) {
        return true;
    }
    if unmerged
        .match_indices('/')
        .any(|(position, _)| asked.contains(&unmerged[..position]))
    {
        return true;
    }
    let below = format!("{unmerged}/");
    asked
        .range(below.as_str()..)
        .next()
        .is_some_and(|path| path.starts_with(&below))
}

// A batch that fails does not say which path it was, so they are retried one by one. A locked index
// is not any path's fault and is usually momentary: there it stops, and the journal will be retried.
fn failed_paths(
    repo: &Path,
    saved: &BTreeMap<String, Saved>,
    paths: &[String],
) -> Result<BTreeSet<String>, RestoreFailure> {
    let Err(failure) = restore(repo, saved, paths) else {
        return Ok(BTreeSet::new());
    };
    if failure.index_locked {
        return Err(failure);
    }
    let mut failed = BTreeSet::new();
    for path in failure.paths {
        match restore(repo, saved, std::slice::from_ref(&path)) {
            Ok(()) => {}
            Err(failure) if failure.index_locked => return Err(failure),
            Err(_) => {
                failed.insert(path);
            }
        }
    }
    Ok(failed)
}

// Restores only what is still as `unstage` left it: anything else is later work by the user,
// or a conflict, and is reported without touching it. A path that already has its entry had not gotten as far as
// being removed or was already restored in an earlier attempt, and does not ask for the index. What `update-index` rejects, such as a file
// that clashes with a folder in the index, is reported all the same: a journal that cannot be finished would
// block all operations, and the report carries mode and hash. With the index locked the
// journal is kept.
fn apply_journal(repo: &Path, dir: &Path) -> Result<RecoveryReport, CommitError> {
    let Some(journal) = read_journal(dir)? else {
        return Ok(RecoveryReport::default());
    };
    let paths: Vec<String> = journal.keys().cloned().collect();
    let conflicts = conflicted(repo, &paths)?;
    let current = read_entries(repo, &paths)?;
    let mut report = RecoveryReport::default();
    let mut saved = BTreeMap::new();
    for (path, entry) in journal {
        let found = &current[&path];
        if conflicts.contains(&path) || (*found != entry.left && *found != entry.restore) {
            report.skipped.push(expected_entry(&path, &entry.restore));
            continue;
        }
        if *found == entry.restore {
            report.restored.push(path);
            continue;
        }
        saved.insert(path, entry.restore);
    }
    let to_restore: Vec<String> = saved.keys().cloned().collect();
    let failed = failed_paths(repo, &saved, &to_restore).map_err(
        |RestoreFailure { paths, detail, .. }| CommitError::Restore {
            paths,
            detail,
            outcome: RECOVERY_OUTCOME.to_string(),
        },
    )?;
    for path in to_restore {
        if failed.contains(&path) {
            report.skipped.push(expected_entry(&path, &saved[&path]));
        } else {
            report.restored.push(path);
        }
    }
    report.restored.sort();
    clear_journal(dir).map_err(|error| {
        CommitError::Journal(format!(
            "The stage recovery finished, but its journal could not be deleted: {error}"
        ))
    })?;
    Ok(report)
}

// Returns to the stage what an interrupted commit excluded. If `git commit` is still alive it touches
// nothing: it rereads the index after `pre-commit`, and what was restored would enter that commit.
pub fn recover(repo: &Path, dir: &Path) -> Result<RecoveryReport, CommitError> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(dir.join(LOCK_FILE));
    let _lock = match file {
        Ok(file) => match try_lock(file)? {
            Some(lock) => Some(lock),
            None => {
                return Ok(RecoveryReport {
                    pending: true,
                    ..Default::default()
                })
            }
        },
        // Without a lock file there has been no commit yet.
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(lock_error(error)),
    };
    apply_journal(repo, dir)
}

fn check_recovery(report: &RecoveryReport) -> Result<(), CommitError> {
    if report.pending {
        return Err(CommitError::CommitRunning);
    }
    if !report.skipped.is_empty() {
        return Err(CommitError::RecoverySkipped(report.skipped.clone()));
    }
    Ok(())
}

// For whatever is going to change the index or the disk outside a commit. What was skipped is reported here: the
// journal is gone and it would not be known again.
pub fn ensure_idle(repo: &Path, dir: &Path) -> Result<(), CommitError> {
    check_recovery(&recover(repo, dir)?)
}

// What the interface asks before a commit, like the checks of
// `GitCheckinHandlerFactory`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "paths", rename_all = "camelCase")]
pub enum CommitWarning {
    Crlf(Vec<String>),
    // The files over the limit, with their size in bytes.
    LargeFiles(Vec<(String, u64)>),
    DetachedHead,
    Rebasing,
    BadFileNames(Vec<BadFileName>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BadNameProblem {
    Reserved,
    InvalidChar,
}

// The first part of a path that Windows would not accept, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BadFileName {
    pub name: String,
    pub problem: BadNameProblem,
}

fn has_crlf(bytes: &[u8]) -> bool {
    !diff::looks_binary(bytes) && bytes.windows(2).any(|pair| pair == b"\r\n")
}

// The disk is looked at first: asking for the stored form costs a copy of the index.
fn stores_crlf(repo: &Path, path: &str) -> bool {
    matches!(hunks::read_current(repo, path), Ok(Some(disk)) if has_crlf(&disk.bytes))
        && matches!(hunks::stored_form(repo, path), Ok(Some(stored)) if has_crlf(&stored))
}

fn request_crlf(repo: &Path, request: &CommitRequest) -> Vec<String> {
    let partial = request.partial.iter();
    let chosen: BTreeSet<&String> = partial.clone().map(|file| &file.path).collect();
    let whole = request.added.iter().filter(|path| !chosen.contains(path));
    whole
        .filter(|path| stores_crlf(repo, path))
        .chain(
            partial
                .filter(|file| has_crlf(&file.content))
                .map(|file| &file.path),
        )
        .cloned()
        .collect()
}

fn staged_crlf(repo: &Path) -> Vec<String> {
    let paths = staged_paths(repo).unwrap_or_default();
    let stores = |path: &String| matches!(hunks::read_index(repo, path), Ok(Some(staged)) if has_crlf(&staged));
    paths.into_iter().filter(stores).collect()
}

fn autocrlf_is_set(repo: &Path) -> bool {
    git::run(repo, &["config", "core.autocrlf"]).is_ok_and(|output| {
        let value = String::from_utf8_lossy(&output).trim().to_lowercase();
        value == "true" || value == "input"
    })
}

// `text` or `crlf` on a file say that git converts it or that its CRLFs are on purpose.
fn without_text_attributes(repo: &Path, paths: Vec<String>) -> Vec<String> {
    let args = git::exact(&["check-attr", "-z", "text", "crlf"]);
    let Ok(output) = git::git_with_paths(repo, false, &args, &paths) else {
        return Vec::new();
    };
    let fields: Vec<&[u8]> = records(&output).collect();
    let marked: BTreeSet<&[u8]> = fields
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|[_, _, value]| *value != b"unspecified")
        .map(|[path, _, _]| *path)
        .collect();
    paths
        .into_iter()
        .filter(|path| !marked.contains(path.as_bytes()))
        .collect()
}

// Like `GitCrlfProblemsDetector`, and also checking that git will really store a CRLF: a
// failure while looking counts as there being nothing to warn about. `candidates` gives what would be stored with CRLF.
fn crlf_warning(
    repo: &Path,
    settings: &Settings,
    candidates: impl FnOnce() -> Vec<String>,
) -> Option<CommitWarning> {
    if !settings.warn_crlf || autocrlf_is_set(repo) {
        return None;
    }
    let paths = without_text_attributes(repo, candidates());
    (!paths.is_empty()).then_some(CommitWarning::Crlf(paths))
}

fn is_detached(repo: &Path) -> bool {
    matches!(
        git::run(repo, &["symbolic-ref", "-q", "HEAD"]),
        Err(GitError::Failed { code: Some(1), .. })
    )
}

fn in_git_dir(repo: &Path, name: &str) -> bool {
    git::git_path(repo, name).is_ok_and(|path| path.exists())
}

// Like `GitDetachedRootCheckinHandler`, with the state of `GitRepositoryReader`: a merge takes precedence over
// the rebase and this one over a detached HEAD, and an interactive rebase does not warn. Git writes `interactive`
// in every rebase with the `merge` engine, so that warning only appears with `--apply`.
fn head_warning(repo: &Path, settings: &Settings) -> Option<CommitWarning> {
    if !settings.warn_detached || in_git_dir(repo, MERGE_HEAD) {
        return None;
    }
    if let Some(dir) = REBASE_DIRS.iter().find(|dir| in_git_dir(repo, dir)) {
        let interactive = in_git_dir(repo, &format!("{dir}/{REBASE_INTERACTIVE}"));
        return (!interactive).then_some(CommitWarning::Rebasing);
    }
    is_detached(repo).then_some(CommitWarning::DetachedHead)
}

pub fn rebase_in_progress(repo: &Path) -> bool {
    REBASE_DIRS.iter().any(|dir| in_git_dir(repo, dir))
}

pub fn merge_in_progress(repo: &Path) -> bool {
    in_git_dir(repo, MERGE_HEAD)
}

// `GitMerger.mergeCommit`: git's message if there is one, otherwise IntelliJ's. Without a half-done merge
// it does nothing: a bare `git commit` would commit whatever was in the index with an old message. A
// `--squash` with conflicts leaves no `MERGE_HEAD`, and its `MERGE_MSG` only carries the comments about the
// conflicts: the commit text is that of `SQUASH_MSG`. With unresolved paths nothing is attempted.
pub fn merge_commit(repo: &Repo) -> Result<(), GitError> {
    let squash = git::git_path(repo.path, SQUASH_MSG)?;
    let merging = merge_in_progress(repo.path);
    if !merging && !squash.exists() {
        return Err(GitError::Io(NO_MERGE.to_string()));
    }
    if !repo.ctx.git(&["ls-files", "--unmerged"])?.stdout.is_empty() {
        return Err(GitError::Io(UNMERGED_PATHS.to_string()));
    }
    let merge_file = git::git_path(repo.path, MERGE_MSG)?;
    let file = if !merging { squash } else { merge_file };
    let fallback;
    let mut args = vec!["-c", MESSAGE_ENCODING, "commit"];
    if file.exists() {
        let path = file
            .to_str()
            .ok_or_else(|| GitError::Io("Invalid path".to_string()))?;
        args.extend(["-F", path]);
    } else {
        let branch = branches::current_name(repo.path)?.unwrap_or_default();
        fallback = format!(
            "Merge branch '{branch}' of {} with conflicts.",
            repo.path.display()
        );
        args.extend(["-m", &fallback]);
    }
    args.push("--");
    repo.ctx.git_loud(&args).map(drop)
}

// `GitCheckinEnvironment.getDefaultMessageFor`: a merge's, and if there is none, a squash's.
pub fn merge_message(repo: &Path) -> Result<Option<String>, GitError> {
    for name in [MERGE_MSG, SQUASH_MSG] {
        let file = git::git_path(repo, name)?;
        if let Ok(bytes) = fs::read(file) {
            return Ok(Some(String::from_utf8_lossy(&bytes).into_owned()));
        }
    }
    Ok(None)
}

const BYTES_PER_MB: u64 = 1024 * 1024;
// `lfs.repositoryformatversion` is what `git lfs install` leaves in the repository's configuration.
const LFS_KEY: &str = "lfs.repositoryformatversion";
const NULL_HASH_DIGIT: u8 = b'0';
const GITLINK_MODE: &str = "160000";

// What `git add` would take of a file on disk. A folder has no size, and a link is stored as the short text
// of its target, so it is not followed.
fn disk_size(repo: &Path, path: &str) -> Option<u64> {
    let meta = fs::symlink_metadata(repo.join(path)).ok()?;
    (!meta.is_dir()).then_some(meta.len())
}

// The whole files by their size on disk, and the ones chosen by lines by the content that goes into the index.
fn request_sizes(repo: &Path, request: &CommitRequest) -> Vec<(String, u64)> {
    let chosen: BTreeSet<&String> = request.partial.iter().map(|file| &file.path).collect();
    let whole = request
        .added
        .iter()
        .filter(|path| !chosen.contains(path))
        .filter_map(|path| Some((path.clone(), disk_size(repo, path)?)));
    let partial = request
        .partial
        .iter()
        .map(|file| (file.path.clone(), file.content.len() as u64));
    whole.chain(partial).collect()
}

// One entry of `git diff --cached --raw -z --no-renames`: `:old-mode new-mode old-hash new-hash status`, and the path.
struct StagedChange {
    status: u8,
    mode: String,
    hash: String,
    path: String,
}

fn staged_changes(repo: &Path) -> Result<Vec<StagedChange>, GitError> {
    let args = ["diff", "--cached", "--raw", "-z", "--no-renames"];
    let output = git::run(repo, &args)?;
    let fields: Vec<&[u8]> = records(&output).collect();
    fields
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let meta = String::from_utf8_lossy(pair[0]);
            let parts: Vec<&str> = meta.trim_start_matches(':').split(' ').collect();
            let [_, mode, _, hash, status] = parts[..] else {
                return Err(GitError::Io(
                    "Invalid output from git diff --cached".to_string(),
                ));
            };
            Ok(StagedChange {
                status: status.bytes().next().unwrap_or(b' '),
                mode: mode.to_string(),
                hash: hash.to_string(),
                path: path_from(pair[1])?,
            })
        })
        .collect()
}

// The commit takes the blobs of the index, whatever the disk has now.
fn staged_sizes(repo: &Path) -> Vec<(String, u64)> {
    let Ok(changes) = staged_changes(repo) else {
        return Vec::new();
    };
    let kept: Vec<&StagedChange> = changes
        .iter()
        .filter(|change| change.status != b'D' && change.mode != GITLINK_MODE)
        .filter(|change| change.hash.bytes().any(|digit| digit != NULL_HASH_DIGIT))
        .collect();
    let input: String = kept
        .iter()
        .map(|change| format!("{}\n", change.hash))
        .collect();
    let args = ["cat-file", "--batch-check=%(objectsize)"];
    let Ok(sizes) = git::run_with_input(repo, &args, input.as_bytes()) else {
        return Vec::new();
    };
    let sizes = String::from_utf8_lossy(&sizes);
    kept.iter()
        .zip(sizes.lines())
        .filter_map(|(change, size)| Some((change.path.clone(), size.parse().ok()?)))
        .collect()
}

// Like `GitLargeFileCheckinHandler`: the files over the limit, unless the repository uses LFS, whose
// filter keeps the real content out of the commit. `sizes` gives every included file with its size.
fn large_files_warning(
    repo: &Path,
    settings: &Settings,
    sizes: impl FnOnce() -> Vec<(String, u64)>,
) -> Option<CommitWarning> {
    let limit = u64::from(settings.large_files_limit_mb) * BYTES_PER_MB;
    if !settings.warn_large_files || limit == 0 {
        return None;
    }
    let large: Vec<(String, u64)> = sizes()
        .into_iter()
        .filter(|(_, size)| *size > limit)
        .collect();
    if large.is_empty() || git::run(repo, &["config", "--get", LFS_KEY]).is_ok() {
        return None;
    }
    Some(CommitWarning::LargeFiles(large))
}

const WINDOWS_INVALID_CHARS: &str = "<>:\"\\|?*";
const WINDOWS_RESERVED_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

// `FileUtil.getNameWithoutExtension`: up to the last dot.
fn without_extension(name: &str) -> &str {
    name.rfind('.').map_or(name, |dot| &name[..dot])
}

// `GitFileNameCheckinHandler.checkFileName`: from the file up to the root, the first name that is reserved or
// has a character Windows does not accept.
fn bad_file_name(path: &str) -> Option<BadFileName> {
    path.rsplit('/').find_map(|name| {
        let base = without_extension(name);
        if (3..=4).contains(&base.chars().count())
            && WINDOWS_RESERVED_NAMES
                .iter()
                .any(|reserved| reserved.eq_ignore_ascii_case(base))
        {
            return Some(BadFileName {
                name: name.to_string(),
                problem: BadNameProblem::Reserved,
            });
        }
        let invalid = |char: char| u32::from(char) <= 31 || WINDOWS_INVALID_CHARS.contains(char);
        name.chars().any(invalid).then(|| BadFileName {
            name: name.to_string(),
            problem: BadNameProblem::InvalidChar,
        })
    })
}

// The paths already in HEAD. Without commits there are none.
fn in_head(repo: &Path, paths: &[String]) -> Result<BTreeSet<String>, GitError> {
    if !git::head_exists(repo)? {
        return Ok(BTreeSet::new());
    }
    let head = git::exact(&["ls-tree", "-r", "-z", "--name-only", "HEAD"]);
    let listed = git::git_with_paths(repo, false, &head, paths)?;
    records(&listed).map(path_from).collect()
}

// `afterPath != null && beforePath != afterPath`: what is not in HEAD is an addition or the target of a rename. The
// ones with a bad name are looked at first, so that git is only asked about those.
fn request_bad_names(repo: &Path, request: &CommitRequest) -> Vec<BadFileName> {
    let partial = request.partial.iter().map(|file| &file.path);
    let candidates: Vec<(String, BadFileName)> = request
        .added
        .iter()
        .chain(partial)
        .filter_map(|path| Some((path.clone(), bad_file_name(path)?)))
        .collect();
    if candidates.is_empty() {
        return Vec::new();
    }
    let paths: Vec<String> = candidates.iter().map(|(path, _)| path.clone()).collect();
    let Ok(existing) = in_head(repo, &paths) else {
        return Vec::new();
    };
    let new = candidates
        .into_iter()
        .filter(|(path, _)| !existing.contains(path));
    new.map(|(_, name)| name).collect()
}

// With `--no-renames` the target of a rename is an addition.
fn staged_bad_names(repo: &Path) -> Vec<BadFileName> {
    let changes = staged_changes(repo).unwrap_or_default();
    let added = changes.iter().filter(|change| change.status == b'A');
    added
        .filter_map(|change| bad_file_name(&change.path))
        .collect()
}

fn file_names_warning(
    settings: &Settings,
    names: impl FnOnce() -> Vec<BadFileName>,
) -> Option<CommitWarning> {
    if cfg!(windows) || !settings.warn_bad_file_names {
        return None;
    }
    let mut unique: Vec<BadFileName> = Vec::new();
    for name in names() {
        if !unique.contains(&name) {
            unique.push(name);
        }
    }
    (!unique.is_empty()).then_some(CommitWarning::BadFileNames(unique))
}

// In the order of the handlers of `GitCheckinHandlerFactory`: CRLF, large files, detached HEAD or rebase, file names.
// The one for the user name does not exist here: git asks for it by itself.
pub fn warnings(repo: &Path, request: &CommitRequest, settings: &Settings) -> Vec<CommitWarning> {
    let crlf = crlf_warning(repo, settings, || request_crlf(repo, request));
    let large = large_files_warning(repo, settings, || request_sizes(repo, request));
    let names = file_names_warning(settings, || request_bad_names(repo, request));
    crlf.into_iter()
        .chain(large)
        .chain(head_warning(repo, settings))
        .chain(names)
        .collect()
}

// Like `warnings`, for the commit of the index as it is.
pub fn staged_warnings(repo: &Path, settings: &Settings) -> Vec<CommitWarning> {
    let crlf = crlf_warning(repo, settings, || staged_crlf(repo));
    let large = large_files_warning(repo, settings, || staged_sizes(repo));
    let names = file_names_warning(settings, || staged_bad_names(repo));
    crlf.into_iter()
        .chain(large)
        .chain(head_warning(repo, settings))
        .chain(names)
        .collect()
}

// `Fix and Commit`: the same `git config --global` that the dialog announces.
pub fn fix_autocrlf(repo: &Path) -> Result<(), GitError> {
    let args = ["config", "--global", "core.autocrlf", RECOMMENDED_AUTOCRLF];
    git::run(repo, &args).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{RepoHarness, TempRepo, DAMAGED_REFS};
    use std::ffi::OsStr;
    use std::fs;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::process::Command;
    use std::thread;
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    // For the tests that do not look at the repository's own data.
    fn commit(repo: &Path, request: &CommitRequest) -> Result<String, CommitError> {
        let dir = TempDir::new().unwrap();
        super::commit(repo, dir.path(), request)
    }

    fn strings(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|path| path.to_string()).collect()
    }

    fn partial(path: &str, content: &[u8]) -> PartialFile {
        PartialFile {
            path: path.to_string(),
            content: content.to_vec(),
        }
    }

    fn partial_request(file: PartialFile) -> CommitRequest {
        CommitRequest {
            partial: vec![file],
            message: "partial".to_string(),
            ..Default::default()
        }
    }

    fn request(added: &[&str]) -> CommitRequest {
        CommitRequest {
            added: strings(added),
            message: "message".to_string(),
            ..Default::default()
        }
    }

    fn write(repo: &TempRepo, path: &str, contents: &[u8]) {
        let target = repo.path().join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, contents).unwrap();
    }

    fn stage(repo: &TempRepo, path: &str, contents: &[u8]) {
        write(repo, path, contents);
        repo.git(&["add", "--", path]);
    }

    fn hook(repo: &TempRepo, name: &str, script: &str) {
        let target = repo.path().join(".git/hooks").join(name);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn index(repo: &TempRepo) -> String {
        repo.git(&["ls-files", "-s"])
    }

    fn status(repo: &TempRepo) -> String {
        repo.git(&["status", "--porcelain"])
    }

    fn head_files(repo: &TempRepo) -> String {
        repo.git(&["ls-tree", "-r", "--name-only", "HEAD"])
    }

    fn at_head(repo: &TempRepo, path: &str) -> String {
        repo.git(&["show", &format!("HEAD:{path}")])
    }

    fn commits(repo: &TempRepo) -> String {
        repo.git(&["rev-list", "--count", "HEAD"])
    }

    fn body(repo: &TempRepo) -> String {
        repo.git(&["log", "-1", "--format=%B"])
    }

    // Checks that `run` fails without changing a single byte of what was in the index.
    fn assert_untouched(
        repo: &TempRepo,
        run: impl FnOnce() -> Result<String, CommitError>,
    ) -> CommitError {
        assert_same_stage(repo, || run().unwrap_err())
    }

    fn assert_same_stage<T>(repo: &TempRepo, run: impl FnOnce() -> T) -> T {
        let (entries, state, head) = (index(repo), status(repo), refs(repo));
        let result = run();
        assert_eq!(index(repo), entries);
        assert_eq!(status(repo), state);
        assert_eq!(refs(repo), head);
        result
    }

    // It also works on a branch without commits, where there is no HEAD to read.
    fn refs(repo: &TempRepo) -> String {
        repo.git(&["for-each-ref"])
    }

    // From the brief.

    #[test]
    fn commit_of_a_file() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        repo.commit("b.txt", b"1\n", "two");
        write(&repo, "a.txt", b"2\n");
        write(&repo, "b.txt", b"2\n");

        let hash = commit(repo.path(), &request(&["a.txt"])).unwrap();

        assert_eq!(repo.git(&["rev-parse", "HEAD"]).trim(), hash);
        assert_eq!(at_head(&repo, "a.txt"), "2\n");
        assert_eq!(at_head(&repo, "b.txt"), "1\n");
        assert_eq!(status(&repo), " M b.txt\n");
    }

    #[test]
    fn keeps_excluded_stage() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        repo.commit("b.txt", b"1\n", "two");
        write(&repo, "a.txt", b"2\n");
        stage(&repo, "b.txt", b"staged\n");
        write(&repo, "b.txt", b"on disk\n");

        commit(repo.path(), &request(&["a.txt"])).unwrap();

        assert_eq!(at_head(&repo, "b.txt"), "1\n");
        assert_eq!(status(&repo), "MM b.txt\n");
        assert_eq!(repo.git(&["show", ":b.txt"]), "staged\n");
        assert_eq!(fs::read(repo.path().join("b.txt")).unwrap(), b"on disk\n");
    }

    #[test]
    fn restores_stage_if_the_hook_fails() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        repo.commit("b.txt", b"1\n", "two");
        repo.commit("c.txt", b"1\n", "three");
        hook(&repo, "pre-commit", "echo rejected >&2\nexit 1");
        write(&repo, "a.txt", b"unstaged\n");
        stage(&repo, "b.txt", b"excluded\n");
        write(&repo, "b.txt", b"excluded and on disk\n");
        stage(&repo, "c.txt", b"selected in stage\n");
        write(&repo, "c.txt", b"selected on disk\n");

        let error = assert_untouched(&repo, || commit(repo.path(), &request(&["a.txt", "c.txt"])));

        assert!(
            matches!(&error, CommitError::Hook(text) if text.contains("rejected")),
            "{error:?}"
        );
        assert_eq!(repo.git(&["show", ":c.txt"]), "selected in stage\n");
    }

    #[test]
    fn error_with_conflicts() {
        let repo = TempRepo::new();
        repo.commit("f.txt", b"base\n", "base");
        repo.git(&["checkout", "-q", "-b", "other"]);
        repo.commit("f.txt", b"other\n", "other");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("f.txt", b"main\n", "main");
        let merge = crate::test_support::git_command()
            .current_dir(repo.path())
            .args(["merge", "--no-edit", "other"])
            .output()
            .unwrap();
        assert!(!merge.status.success());

        let error = assert_untouched(&repo, || commit(repo.path(), &request(&["f.txt"])));

        assert_eq!(error, CommitError::Unmerged);
    }

    #[test]
    fn deleted_excluded_with_ad_state() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        write(&repo, "a.txt", b"2\n");
        stage(&repo, "new.txt", b"staged\n");
        fs::remove_file(repo.path().join("new.txt")).unwrap();
        assert!(status(&repo).contains("AD new.txt"));

        commit(repo.path(), &request(&["a.txt"])).unwrap();

        assert_eq!(head_files(&repo), "a.txt\n");
        assert_eq!(status(&repo), "AD new.txt\n");
        assert_eq!(repo.git(&["show", ":new.txt"]), "staged\n");
    }

    #[test]
    fn partial_commit() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n2\n", "one");
        write(&repo, "a.txt", b"1\n2\n3\n4\n");
        let request = CommitRequest {
            partial: vec![PartialFile {
                path: "a.txt".to_string(),
                content: b"1\n2\n3\n".to_vec(),
            }],
            message: "partial".to_string(),
            ..Default::default()
        };

        commit(repo.path(), &request).unwrap();

        assert_eq!(at_head(&repo, "a.txt"), "1\n2\n3\n");
        assert_eq!(
            fs::read(repo.path().join("a.txt")).unwrap(),
            b"1\n2\n3\n4\n"
        );
        assert_eq!(status(&repo), " M a.txt\n");
    }

    // `content` is already in the form git stores: it is not filtered again, neither by `autocrlf` nor
    // by a clean filter.
    #[test]
    fn partial_content_is_stored_as_is() {
        let repo = TempRepo::new();
        repo.git(&["config", "core.autocrlf", "false"]);
        repo.commit("a.txt", b"1\r\n2\r\n", "one");
        repo.git(&["config", "core.autocrlf", "input"]);
        repo.git(&["config", "filter.upper.clean", "tr a-z A-Z"]);
        fs::write(repo.path().join(".gitattributes"), "b.txt filter=upper\n").unwrap();
        write(&repo, "a.txt", b"1\r\n2\r\n3\r\n4\r\n");
        write(&repo, "b.txt", b"hello\n");
        let request = CommitRequest {
            partial: vec![
                PartialFile {
                    path: "a.txt".to_string(),
                    content: b"1\r\n2\r\n3\r\n".to_vec(),
                },
                PartialFile {
                    path: "b.txt".to_string(),
                    content: b"hello\n".to_vec(),
                },
            ],
            message: "partial".to_string(),
            ..Default::default()
        };

        commit(repo.path(), &request).unwrap();

        assert_eq!(at_head(&repo, "a.txt"), "1\r\n2\r\n3\r\n");
        assert_eq!(at_head(&repo, "b.txt"), "hello\n");
    }

    #[test]
    fn partial_keeps_executable_permission() {
        let repo = TempRepo::new();
        repo.commit("run.sh", b"1\n", "one");
        fs::set_permissions(
            repo.path().join("run.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        repo.git(&["add", "run.sh"]);
        repo.git(&["commit", "-q", "-m", "executable"]);
        write(&repo, "run.sh", b"1\n2\n3\n");
        let request = CommitRequest {
            partial: vec![PartialFile {
                path: "run.sh".to_string(),
                content: b"1\n2\n".to_vec(),
            }],
            message: "partial".to_string(),
            ..Default::default()
        };

        commit(repo.path(), &request).unwrap();

        assert!(repo
            .git(&["ls-tree", "HEAD", "run.sh"])
            .starts_with("100755 "));
    }

    #[test]
    fn amend() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        repo.commit("b.txt", b"1\n", "two");
        let before = repo.git(&["rev-parse", "HEAD"]);
        let count = commits(&repo);
        write(&repo, "b.txt", b"2\n");
        let mut request = request(&["b.txt"]);
        request.message = "two fixed".to_string();
        request.options.amend = true;

        commit(repo.path(), &request).unwrap();

        assert_eq!(commits(&repo), count);
        assert_ne!(repo.git(&["rev-parse", "HEAD"]), before);
        assert_eq!(body(&repo).trim(), "two fixed");
        assert_eq!(at_head(&repo, "b.txt"), "2\n");
    }

    #[test]
    fn amend_of_only_the_message_keeps_excluded_stage() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        stage(&repo, "a.txt", b"staged\n");
        let mut request = request(&[]);
        request.message = "other message".to_string();
        request.options.amend = true;

        commit(repo.path(), &request).unwrap();

        assert_eq!(body(&repo).trim(), "other message");
        assert_eq!(at_head(&repo, "a.txt"), "1\n");
        assert_eq!(status(&repo), "M  a.txt\n");
    }

    #[test]
    fn sign_off() {
        let repo = TempRepo::new();
        write(&repo, "a.txt", b"1\n");
        let mut request = request(&["a.txt"]);
        request.options.sign_off = true;

        commit(repo.path(), &request).unwrap();

        assert!(body(&repo).contains("Signed-off-by: Test <test@example.com>"));
    }

    #[test]
    fn author() {
        let repo = TempRepo::new();
        write(&repo, "a.txt", b"1\n");
        let mut request = request(&["a.txt"]);
        request.options.author = Some("Another Person <other@example.com>".to_string());

        commit(repo.path(), &request).unwrap();

        assert_eq!(
            repo.git(&["log", "-1", "--format=%an <%ae>|%cn"]).trim(),
            "Another Person <other@example.com>|Test"
        );
    }

    #[test]
    fn no_changes() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");

        let error = assert_untouched(&repo, || commit(repo.path(), &request(&["a.txt"])));

        assert_eq!(error, CommitError::NothingToCommit);
    }

    #[test]
    fn no_changes_in_the_selection_even_if_there_are_others() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        repo.commit("b.txt", b"1\n", "two");
        stage(&repo, "b.txt", b"staged\n");
        write(&repo, "loose.txt", b"unversioned\n");

        let error = assert_untouched(&repo, || commit(repo.path(), &request(&["a.txt"])));

        assert_eq!(error, CommitError::NothingToCommit);
    }

    #[test]
    fn message_with_quotes_and_accents() {
        for message in [
            "«año» \"double\" 'single' `$(no)`\n\nbody ñ",
            "-m is not an option",
        ] {
            let repo = TempRepo::new();
            write(&repo, "a.txt", b"1\n");
            let mut request = request(&["a.txt"]);
            request.message = message.to_string();

            commit(repo.path(), &request).unwrap();

            assert_eq!(body(&repo).trim_end(), message);
        }
    }

    #[test]
    fn path_that_looks_like_an_option() {
        let repo = TempRepo::new();
        repo.commit("-deleted.txt", b"1\n", "one");
        fs::remove_file(repo.path().join("-deleted.txt")).unwrap();
        write(&repo, "-file.txt", b"1\n");
        stage(&repo, "-excluded.txt", b"1\n");
        let mut request = request(&["-file.txt"]);
        request.removed = strings(&["-deleted.txt"]);

        commit(repo.path(), &request).unwrap();

        assert_eq!(head_files(&repo), "-file.txt\n");
        assert_eq!(status(&repo), "A  -excluded.txt\n");
    }

    // From the task's decisions.

    #[test]
    fn first_commit_of_the_repository() {
        let repo = TempRepo::new();
        write(&repo, "a.txt", b"1\n");
        stage(&repo, "excluded.txt", b"staged\n");
        write(&repo, "excluded.txt", b"on disk\n");
        stage(&repo, "no-copy.txt", b"only in stage\n");
        fs::remove_file(repo.path().join("no-copy.txt")).unwrap();

        commit(repo.path(), &request(&["a.txt"])).unwrap();

        assert_eq!(head_files(&repo), "a.txt\n");
        assert_eq!(status(&repo), "AM excluded.txt\nAD no-copy.txt\n");
        assert_eq!(repo.git(&["show", ":excluded.txt"]), "staged\n");
    }

    #[test]
    fn first_failing_commit_leaves_the_stage_unchanged() {
        let repo = TempRepo::new();
        hook(&repo, "pre-commit", "exit 1");
        write(&repo, "a.txt", b"1\n");
        stage(&repo, "b.txt", b"selected\n");
        stage(&repo, "excluded.txt", b"staged\n");

        let error = assert_untouched(&repo, || commit(repo.path(), &request(&["a.txt", "b.txt"])));

        assert!(matches!(error, CommitError::Hook(_)), "{error:?}");
    }

    #[test]
    fn conflicts_on_a_branch_without_commits() {
        let repo = TempRepo::new();
        write(&repo, "base", b"content\n");
        let blob = repo.git(&["hash-object", "-w", "base"]);
        let entry = format!("100644 {} 2\tf.txt\n", blob.trim());
        let mut child = crate::test_support::git_command()
            .current_dir(repo.path())
            .args(["update-index", "--index-info"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        std::io::Write::write_all(child.stdin.as_mut().unwrap(), entry.as_bytes()).unwrap();
        assert!(child.wait().unwrap().success());

        let error = commit(repo.path(), &request(&["base"])).unwrap_err();

        assert_eq!(error, CommitError::Unmerged);
    }

    fn index_info(repo: &TempRepo, entries: &str) {
        let mut child = crate::test_support::git_command()
            .current_dir(repo.path())
            .args(["update-index", "--add", "--index-info"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        std::io::Write::write_all(child.stdin.as_mut().unwrap(), entries.as_bytes()).unwrap();
        assert!(child.wait().unwrap().success());
    }

    #[test]
    fn conflict_in_a_child_folder_is_seen_with_a_folder_pathspec() {
        let repo = TempRepo::new();
        repo.conflict("d/f.txt");
        let ask = |path: &str| conflicted(repo.path(), &[path.to_string()]).unwrap();

        // The folder that contains it, with or without a slash, and the file itself.
        assert_eq!(ask("d").len(), 1);
        assert_eq!(ask("d/").len(), 1);
        assert_eq!(ask("d/f.txt").len(), 1);
        assert!(ask("other").is_empty());
        assert!(ask("d/f").is_empty());

        // An unresolved file that is an ancestor of the requested path: `git add d/f` would remove it from the
        // index, and the conflict would be lost without anyone resolving it.
        let ancestor = TempRepo::new();
        write(&ancestor, "b", b"x\n");
        let blob = ancestor.git(&["hash-object", "-w", "b"]);
        let blob = blob.trim();
        index_info(
            &ancestor,
            &format!("100644 {blob} 2\td\n100644 {blob} 0\td/f\n"),
        );
        let found = conflicted(ancestor.path(), &["d/f".to_string()]).unwrap();
        assert_eq!(found.into_iter().collect::<Vec<_>>(), ["d"]);
    }

    #[test]
    fn keeps_excluded_deletion() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        repo.commit("b.txt", b"1\n", "two");
        write(&repo, "a.txt", b"2\n");
        repo.git(&["rm", "-q", "b.txt"]);

        commit(repo.path(), &request(&["a.txt"])).unwrap();

        assert_eq!(head_files(&repo), "a.txt\nb.txt\n");
        assert_eq!(status(&repo), "D  b.txt\n");
    }

    #[test]
    fn keeps_excluded_deletion_if_the_hook_fails() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        repo.commit("b.txt", b"1\n", "two");
        hook(&repo, "pre-commit", "exit 1");
        write(&repo, "a.txt", b"2\n");
        repo.git(&["rm", "-q", "b.txt"]);

        assert_untouched(&repo, || commit(repo.path(), &request(&["a.txt"])));
    }

    #[test]
    fn keeps_excluded_rename() {
        let repo = TempRepo::new();
        repo.commit("old.txt", b"enough content to detect the rename\n", "one");
        repo.commit("a.txt", b"1\n", "two");
        repo.git(&["mv", "old.txt", "new.txt"]);
        write(&repo, "a.txt", b"2\n");
        assert!(status(&repo).contains("R  old.txt -> new.txt"));

        commit(repo.path(), &request(&["a.txt"])).unwrap();

        assert_eq!(head_files(&repo), "a.txt\nold.txt\n");
        assert_eq!(status(&repo), "R  old.txt -> new.txt\n");
    }

    #[test]
    fn excluded_file_turned_into_folder() {
        let repo = TempRepo::new();
        repo.commit("thing", b"file\n", "one");
        repo.commit("a.txt", b"1\n", "two");
        repo.git(&["rm", "-q", "thing"]);
        stage(&repo, "thing/inside.txt", b"now a folder\n");
        write(&repo, "a.txt", b"2\n");
        let entries = status(&repo);

        commit(repo.path(), &request(&["a.txt"])).unwrap();

        assert_eq!(head_files(&repo), "a.txt\nthing\n");
        assert_eq!(status(&repo), entries.replace(" M a.txt\n", ""));
    }

    #[test]
    fn keeps_excluded_mode_and_link() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        repo.commit("run.sh", b"1\n", "two");
        write(&repo, "a.txt", b"2\n");
        fs::set_permissions(
            repo.path().join("run.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        symlink("a.txt", repo.path().join("link")).unwrap();
        repo.git(&["add", "run.sh", "link"]);
        let staged = repo.git(&["ls-files", "-s", "--", "run.sh", "link"]);

        commit(repo.path(), &request(&["a.txt"])).unwrap();

        assert_eq!(
            repo.git(&["ls-files", "-s", "--", "run.sh", "link"]),
            staged
        );
        assert_eq!(head_files(&repo), "a.txt\nrun.sh\n");
    }

    #[test]
    fn paths_are_literals_not_patterns() {
        let repo = TempRepo::new();
        for name in [":(exclude)a.txt", "*.txt", "a,two.txt"] {
            repo.commit(name, b"1\n", name);
        }
        repo.commit("a.txt", b"1\n", "a");
        repo.commit("b.txt", b"1\n", "b");
        write(&repo, ":(exclude)a.txt", b"2\n");
        stage(&repo, "*.txt", b"excluded\n");
        stage(&repo, "a,two.txt", b"excluded\n");
        stage(&repo, "a.txt", b"excluded\n");
        write(&repo, "b.txt", b"unstaged\n");

        commit(repo.path(), &request(&[":(exclude)a.txt"])).unwrap();

        assert_eq!(at_head(&repo, ":(exclude)a.txt"), "2\n");
        assert_eq!(at_head(&repo, "a.txt"), "1\n");
        assert_eq!(
            status(&repo),
            "M  *.txt\nM  a,two.txt\nM  a.txt\n M b.txt\n"
        );
    }

    #[test]
    fn paths_outside_the_repository() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        let outside = TempDir::new().unwrap();
        symlink(outside.path(), repo.path().join("outside")).unwrap();
        repo.git(&["add", "outside"]);
        repo.git(&["commit", "-q", "-m", "link"]);
        write(&repo, "a.txt", b"2\n");
        let bad = [
            "../x",
            "/etc/passwd",
            ".git/config",
            "outside/x",
            "a/../../x",
            "",
        ];

        for path in bad {
            let cases = [
                CommitRequest {
                    added: strings(&[path]),
                    ..request(&["a.txt"])
                },
                CommitRequest {
                    removed: strings(&[path]),
                    ..request(&["a.txt"])
                },
                CommitRequest {
                    partial: vec![PartialFile {
                        path: path.to_string(),
                        content: b"x".to_vec(),
                    }],
                    ..request(&["a.txt"])
                },
            ];
            for request in &cases {
                let error = assert_untouched(&repo, || commit(repo.path(), request));
                assert_eq!(error, CommitError::OutsideRepository(path.to_string()));
            }
        }
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    #[test]
    fn unreadable_file_leaves_no_half_done_commit() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        repo.commit("locked.txt", b"1\n", "two");
        stage(&repo, "excluded.txt", b"staged\n");
        write(&repo, "a.txt", b"2\n");
        write(&repo, "locked.txt", b"2\n");
        write(&repo, "new.txt", b"1\n");
        let locked = repo.path().join("locked.txt");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        for selected in [["a.txt", "locked.txt"], ["new.txt", "locked.txt"]] {
            let error = assert_untouched(&repo, || commit(repo.path(), &request(&selected)));

            assert!(
                matches!(error, CommitError::Git(GitError::Failed { .. })),
                "{error:?}"
            );
        }
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).unwrap();
    }

    #[test]
    fn nonexistent_path_leaves_no_half_done_commit() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        write(&repo, "a.txt", b"2\n");
        stage(&repo, "excluded.txt", b"staged\n");

        let error = assert_untouched(&repo, || {
            commit(repo.path(), &request(&["a.txt", "missing"]))
        });

        assert!(
            matches!(error, CommitError::Git(GitError::Failed { .. })),
            "{error:?}"
        );
    }

    #[test]
    fn hook_includes_stdout_and_stderr() {
        for name in ["pre-commit", "commit-msg"] {
            let repo = TempRepo::new();
            write(&repo, "a.txt", b"1\n");
            hook(
                &repo,
                name,
                "echo from-stdout\necho from-stderr >&2\nexit 1",
            );

            let error = commit(repo.path(), &request(&["a.txt"])).unwrap_err();

            let CommitError::Hook(text) = &error else {
                panic!("{error:?}")
            };
            assert!(text.contains("from-stdout"), "{text}");
            assert!(text.contains("from-stderr"), "{text}");
        }
    }

    #[test]
    fn hook_in_core_hooks_path() {
        let repo = TempRepo::new();
        let hooks = TempDir::new().unwrap();
        let script = hooks.path().join("pre-commit");
        fs::write(&script, "#!/bin/sh\nexit 1\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        repo.git(&["config", "core.hooksPath", hooks.path().to_str().unwrap()]);
        write(&repo, "a.txt", b"1\n");

        let error = commit(repo.path(), &request(&["a.txt"])).unwrap_err();

        assert!(matches!(error, CommitError::Hook(_)), "{error:?}");
    }

    #[test]
    fn skip_hooks_skips_the_hook() {
        let repo = TempRepo::new();
        hook(&repo, "pre-commit", "exit 1");
        write(&repo, "a.txt", b"1\n");
        let mut request = request(&["a.txt"]);
        request.options.skip_hooks = true;

        commit(repo.path(), &request).unwrap();

        assert_eq!(head_files(&repo), "a.txt\n");
    }

    #[test]
    fn hook_saying_no_changes_with_content_in_stage_is_a_hook() {
        let repo = TempRepo::new();
        hook(
            &repo,
            "pre-commit",
            "echo 'No changes detected by the linter'\nexit 1",
        );
        write(&repo, "a.txt", b"1\n");

        let error = commit(repo.path(), &request(&["a.txt"])).unwrap_err();

        assert!(matches!(error, CommitError::Hook(_)), "{error:?}");
    }

    #[test]
    fn failure_without_a_hook_is_not_attributed_to_a_hook() {
        let repo = TempRepo::new();
        write(&repo, "a.txt", b"1\n");
        fs::create_dir_all(repo.path().join(".git/hooks")).unwrap();
        fs::write(repo.path().join(".git/hooks/pre-commit"), "#!/bin/sh\n").unwrap();
        let mut invalid = request(&["a.txt"]);
        invalid.options.author = Some("no format".to_string());

        let inactive = commit(repo.path(), &invalid).unwrap_err();
        hook(&repo, "pre-commit", "exit 0");
        invalid.options.skip_hooks = true;
        let skipped = commit(repo.path(), &invalid).unwrap_err();

        assert!(
            matches!(inactive, CommitError::Git(GitError::Failed { .. })),
            "{inactive:?}"
        );
        assert!(
            matches!(skipped, CommitError::Git(GitError::Failed { .. })),
            "{skipped:?}"
        );
    }

    #[test]
    fn the_gpg_failure_still_reads_could_not_sign_the_commit() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "unsigned");
        stage(&repo, "excluded.txt", b"staged\n");
        write(&repo, "a.txt", b"2\n");
        repo.git(&["config", "commit.gpgsign", "true"]);
        repo.git(&["config", "gpg.program", "/usr/bin/false"]);

        let error = assert_untouched(&repo, || commit(repo.path(), &request(&["a.txt"])));

        assert!(
            matches!(&error, CommitError::Gpg(text) if text.contains("gpg failed to sign the data")),
            "{error:?}"
        );
        assert!(
            error.to_string().starts_with("Could not sign the commit: error: gpg failed to sign the data"),
            "{error}"
        );
    }

    #[test]
    fn cleanup_strips_comments_only_if_requested() {
        for (cleanup, expected) in [(false, "one\n#two\n"), (true, "one\n")] {
            let repo = TempRepo::new();
            write(&repo, "a.txt", b"1\n");
            let mut request = request(&["a.txt"]);
            request.message = "one\n#two\n".to_string();
            request.options.cleanup = cleanup;

            commit(repo.path(), &request).unwrap();

            assert_eq!(
                repo.git(&["log", "-1", "--format=%B"]).trim_end(),
                expected.trim_end()
            );
        }
    }

    #[test]
    fn temporary_message_outside_the_repository_and_deleted() {
        for code in [0, 1] {
            let repo = TempRepo::new();
            let holder = TempDir::new().unwrap();
            let record = holder.path().join("command");
            let script = format!(
                "ps -o command= -p $PPID > '{}'\nexit {code}",
                record.display()
            );
            hook(&repo, "pre-commit", &script);
            write(&repo, "a.txt", b"1\n");

            let result = commit(repo.path(), &request(&["a.txt"]));

            assert_eq!(result.is_ok(), code == 0);
            let line = fs::read_to_string(&record).unwrap();
            let file = line.split("-F ").nth(1).unwrap().split(' ').next().unwrap();
            assert!(file.starts_with('/'), "{line}");
            assert!(!Path::new(file).starts_with(fs::canonicalize(repo.path()).unwrap()));
            assert!(!Path::new(file).exists(), "{file}");
        }
    }

    #[test]
    fn failing_hook_with_partial_leaves_the_index_unchanged() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n2\n", "one");
        hook(&repo, "pre-commit", "exit 1");
        stage(&repo, "a.txt", b"1\n2\nstaged\n");
        write(&repo, "a.txt", b"1\n2\n3\n4\n");
        let request = CommitRequest {
            partial: vec![PartialFile {
                path: "a.txt".to_string(),
                content: b"1\n2\n3\n".to_vec(),
            }],
            message: "partial".to_string(),
            ..Default::default()
        };

        let error = assert_untouched(&repo, || commit(repo.path(), &request));

        assert!(matches!(error, CommitError::Hook(_)), "{error:?}");
        assert_eq!(
            fs::read(repo.path().join("a.txt")).unwrap(),
            b"1\n2\n3\n4\n"
        );
    }

    #[test]
    fn partial_with_excluded_stage() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n2\n", "one");
        repo.commit("b.txt", b"1\n", "two");
        write(&repo, "a.txt", b"1\n2\n3\n4\n");
        stage(&repo, "b.txt", b"staged\n");
        let request = CommitRequest {
            added: strings(&["a.txt"]),
            partial: vec![PartialFile {
                path: "a.txt".to_string(),
                content: b"1\n2\n3\n".to_vec(),
            }],
            message: "partial".to_string(),
            ..Default::default()
        };

        commit(repo.path(), &request).unwrap();

        assert_eq!(at_head(&repo, "a.txt"), "1\n2\n3\n");
        assert_eq!(at_head(&repo, "b.txt"), "1\n");
        assert_eq!(status(&repo), " M a.txt\nM  b.txt\n");
    }

    #[test]
    fn partial_without_index_entry_takes_the_mode_from_disk() {
        let repo = TempRepo::new();
        repo.commit("base.txt", b"1\n", "one");
        write(&repo, "new.sh", b"1\n2\n");
        let script = repo.path().join("new.sh");
        fs::set_permissions(script, fs::Permissions::from_mode(0o755)).unwrap();
        let request = CommitRequest {
            partial: vec![PartialFile {
                path: "new.sh".to_string(),
                content: b"1\n".to_vec(),
            }],
            message: "partial".to_string(),
            ..Default::default()
        };

        commit(repo.path(), &request).unwrap();

        assert!(repo
            .git(&["ls-tree", "HEAD", "new.sh"])
            .starts_with("100755 "));
    }

    #[test]
    fn partial_of_a_rename() {
        let repo = TempRepo::new();
        repo.commit("old.sh", b"1\n2\n", "one");
        repo.git(&["mv", "old.sh", "new.sh"]);
        write(&repo, "new.sh", b"1\n2\n3\n4\n");
        let request = CommitRequest {
            removed: strings(&["old.sh"]),
            partial: vec![PartialFile {
                path: "new.sh".to_string(),
                content: b"1\n2\n3\n".to_vec(),
            }],
            message: "partial".to_string(),
            ..Default::default()
        };

        commit(repo.path(), &request).unwrap();

        assert_eq!(head_files(&repo), "new.sh\n");
        assert_eq!(at_head(&repo, "new.sh"), "1\n2\n3\n");
        assert_eq!(status(&repo), " M new.sh\n");
    }

    #[test]
    fn deletion_under_a_folder_that_is_now_a_file() {
        let repo = TempRepo::new();
        repo.commit("thing/sub/x.txt", b"1\n", "one");
        fs::remove_dir_all(repo.path().join("thing")).unwrap();
        write(&repo, "thing", b"now file\n");
        let mut request = request(&["thing"]);
        request.removed = strings(&["thing/sub/x.txt"]);

        commit(repo.path(), &request).unwrap();

        assert_eq!(head_files(&repo), "thing\n");
        assert_eq!(status(&repo), "");
    }

    #[test]
    fn selection_with_ignored_file() {
        let repo = TempRepo::new();
        repo.commit(".gitignore", b"*.log\n", "one");
        write(&repo, "x.log", b"1\n");

        commit(repo.path(), &request(&["x.log"])).unwrap();

        assert_eq!(head_files(&repo), ".gitignore\nx.log\n");
    }

    #[test]
    fn path_in_added_and_removed_counts_as_added() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        write(&repo, "a.txt", b"2\n");
        let mut request = request(&["a.txt"]);
        request.removed = strings(&["a.txt"]);

        commit(repo.path(), &request).unwrap();

        assert_eq!(at_head(&repo, "a.txt"), "2\n");
    }

    #[test]
    fn selected_intent_to_add_comes_back_if_the_hook_fails() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        hook(&repo, "pre-commit", "exit 1");
        write(&repo, "new.txt", b"1\n");
        repo.git(&["add", "-N", "new.txt"]);

        let error = assert_untouched(&repo, || commit(repo.path(), &request(&["new.txt"])));

        assert!(matches!(error, CommitError::Hook(_)), "{error:?}");
        assert_eq!(status(&repo), " A new.txt\n");
    }

    #[test]
    fn commit_rename_only_case() {
        let repo = TempRepo::new();
        repo.commit("readme.txt", b"1\n", "one");
        let mut request = request(&["README.txt"]);
        request.removed = strings(&["readme.txt"]);

        // By `git mv`, with the index already renamed.
        repo.git(&["mv", "readme.txt", "README.txt"]);
        commit(repo.path(), &request).unwrap();
        assert_eq!(head_files(&repo), "README.txt\n");

        // Only on disk, with the old name still in the index.
        let mut back = CommitRequest {
            added: strings(&["readme.txt"]),
            removed: strings(&["README.txt"]),
            ..request.clone()
        };
        back.message = "back".to_string();
        fs::rename(
            repo.path().join("README.txt"),
            repo.path().join("readme.txt"),
        )
        .unwrap();
        commit(repo.path(), &back).unwrap();
        assert_eq!(head_files(&repo), "readme.txt\n");
        assert_eq!(status(&repo), "");
    }

    #[test]
    fn excluded_rename_only_case() {
        let repo = TempRepo::new();
        repo.commit("readme.txt", b"1\n", "one");
        repo.commit("a.txt", b"1\n", "two");
        repo.git(&["mv", "readme.txt", "README.txt"]);
        write(&repo, "a.txt", b"2\n");
        let entries = repo.git(&["ls-files", "-s", "--", "README.txt"]);

        commit(repo.path(), &request(&["a.txt"])).unwrap();

        assert_eq!(head_files(&repo), "a.txt\nreadme.txt\n");
        assert_eq!(repo.git(&["ls-files", "-s", "--", "README.txt"]), entries);
        assert_eq!(repo.git(&["ls-files", "--", "readme.txt"]), "");
    }

    #[test]
    fn rename_only_case_comes_back_if_the_hook_fails() {
        let repo = TempRepo::new();
        repo.commit("readme.txt", b"1\n", "one");
        hook(&repo, "pre-commit", "exit 1");
        fs::rename(
            repo.path().join("readme.txt"),
            repo.path().join("README.txt"),
        )
        .unwrap();
        let mut request = request(&["README.txt"]);
        request.removed = strings(&["readme.txt"]);

        let error = assert_untouched(&repo, || commit(repo.path(), &request));

        assert!(matches!(error, CommitError::Hook(_)), "{error:?}");
    }

    const NFD_NAME: &str = "cafe\u{301}.txt";
    const NFD_EXCLUDED: &str = "n\u{303}u.txt";

    // As `status` gives the paths: with the bytes of the index.
    fn index_paths(repo: &TempRepo) -> Vec<String> {
        repo.git(&["-c", "core.quotepath=off", "ls-files", "-z"])
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn nfd_paths_selected_and_excluded() {
        for precompose in ["true", "false"] {
            let repo = TempRepo::new();
            repo.git(&["config", "core.precomposeunicode", precompose]);
            repo.commit("base.txt", b"1\n", "base");
            write(&repo, NFD_NAME, b"1\n");
            stage(&repo, NFD_EXCLUDED, b"staged\n");
            repo.git(&["add", "--", NFD_NAME]);
            let names = index_paths(&repo);
            let selected = names.iter().find(|name| name.starts_with("caf")).unwrap();
            let excluded = names
                .iter()
                .find(|name| name.starts_with('n') || name.starts_with('ñ'))
                .unwrap();
            repo.git(&["rm", "-q", "--cached", "--", selected]);

            commit(repo.path(), &request(&[selected])).unwrap();

            assert_eq!(index_paths(&repo), names);
            let files = repo.git(&[
                "-c",
                "core.quotepath=off",
                "ls-tree",
                "-r",
                "--name-only",
                "HEAD",
            ]);
            assert_eq!(files, format!("base.txt\n{selected}\n"), "{precompose}");
            assert!(!files.contains(excluded.as_str()));
            let staged = repo.git(&[
                "-c",
                "core.quotepath=off",
                "diff",
                "--cached",
                "--name-only",
            ]);
            assert_eq!(staged, format!("{excluded}\n"), "{precompose}");
        }
    }

    // Git converts the arguments to NFC with `core.precomposeunicode`, and an NFD entry of the
    // index (a clone, for example) would not be removed from the stage and would enter the commit.
    #[test]
    fn excluded_nfd_entry_with_precompose_does_not_enter_the_commit() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        repo.git(&["config", "core.precomposeunicode", "true"]);
        write(&repo, "content", b"staged\n");
        let blob = repo.git(&["hash-object", "-w", "content"]);
        fs::remove_file(repo.path().join("content")).unwrap();
        let info = format!("100644,{},{NFD_NAME}", blob.trim());
        repo.git(&[
            "-c",
            "core.precomposeunicode=false",
            "update-index",
            "--add",
            "--cacheinfo",
            &info,
        ]);
        write(&repo, "a.txt", b"2\n");
        let entries = index_paths(&repo);

        commit(repo.path(), &request(&["a.txt"])).unwrap();

        assert_eq!(head_files(&repo), "a.txt\n");
        assert_eq!(index_paths(&repo), entries);
    }

    #[test]
    fn warns_if_it_cannot_restore_after_the_commit_fails() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        hook(
            &repo,
            "pre-commit",
            "touch .git/index.lock\necho rejected\nexit 1",
        );
        stage(&repo, "excluded.txt", b"staged\n");
        write(&repo, "a.txt", b"2\n");
        let blob = repo.git(&["rev-parse", ":excluded.txt"]);

        let error = commit(repo.path(), &request(&["a.txt"])).unwrap_err();

        fs::remove_file(repo.path().join(".git/index.lock")).unwrap();
        let CommitError::Restore {
            paths,
            detail,
            outcome,
        } = &error
        else {
            panic!("{error:?}")
        };
        assert_eq!(paths, &strings(&["a.txt", "excluded.txt"]));
        assert!(detail.contains(blob.trim()), "{detail}");
        assert!(outcome.contains("rejected"), "{outcome}");
        repo.git(&["cat-file", "-e", blob.trim()]);
        let message = error.to_string();
        assert!(
            message.contains("excluded.txt") && message.contains("rejected"),
            "{message}"
        );
    }

    #[test]
    fn warns_if_it_cannot_restore_after_the_commit_is_created() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        hook(&repo, "post-commit", "touch .git/index.lock");
        stage(&repo, "excluded.txt", b"staged\n");
        write(&repo, "a.txt", b"2\n");
        let hash_before = repo.git(&["rev-parse", "HEAD"]);

        let error = commit(repo.path(), &request(&["a.txt"])).unwrap_err();

        fs::remove_file(repo.path().join(".git/index.lock")).unwrap();
        let head = repo.git(&["rev-parse", "HEAD"]);
        assert_ne!(head, hash_before);
        let CommitError::Restore { paths, outcome, .. } = &error else {
            panic!("{error:?}")
        };
        assert_eq!(paths, &strings(&["excluded.txt"]));
        assert!(outcome.contains(head.trim()), "{outcome}");
    }

    #[test]
    fn non_utf8_path_in_stage_touches_nothing() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        write(&repo, "content", b"staged\n");
        let blob = repo.git(&["hash-object", "-w", "content"]);
        fs::remove_file(repo.path().join("content")).unwrap();
        let mut info = format!("100644,{},", blob.trim()).into_bytes();
        info.extend_from_slice(b"caf\xe9.txt");
        let added = crate::test_support::git_command()
            .current_dir(repo.path())
            .args(["update-index", "--add", "--cacheinfo"])
            .arg(OsStr::from_bytes(&info))
            .status()
            .unwrap();
        assert!(added.success());
        write(&repo, "a.txt", b"2\n");

        let error = assert_untouched(&repo, || commit(repo.path(), &request(&["a.txt"])));

        assert!(
            matches!(error, CommitError::Git(GitError::Io(_))),
            "{error:?}"
        );
    }

    #[test]
    fn damaged_branch_touches_nothing() {
        for contents in DAMAGED_REFS {
            let repo = TempRepo::new();
            repo.commit("a.txt", b"1\n", "one");
            stage(&repo, "excluded.txt", b"staged\n");
            write(&repo, "a.txt", b"2\n");
            let entries = index(&repo);
            fs::write(repo.path().join(".git/refs/heads/main"), contents).unwrap();

            let result = commit(repo.path(), &request(&["a.txt"]));

            assert!(
                matches!(result, Err(CommitError::Git(_))),
                "{contents:?}: {result:?}"
            );
            assert_eq!(index(&repo), entries, "{contents:?}");
        }
    }

    #[test]
    fn message_in_utf8_even_if_the_repository_uses_another_encoding() {
        let repo = TempRepo::new();
        repo.git(&["config", "i18n.commitEncoding", "ISO-8859-1"]);
        write(&repo, "a.txt", b"1\n");
        let mut request = request(&["a.txt"]);
        request.message = "año".to_string();

        commit(repo.path(), &request).unwrap();

        let body = repo.git(&["log", "-1", "--encoding=UTF-8", "--format=%B"]);
        assert_eq!(body.trim(), "año");
    }

    // For `docs/reports/phase-1-commands.md`: records the commands of a partial commit with an
    // excluded stage, running the tests with `scripts/git-wrapper.sh` as `git`.
    #[test]
    #[ignore = "only to record commands with scripts/git-wrapper.sh"]
    fn trace_of_partial_commit_with_excluded_stage() {
        let repo = TempRepo::new();
        repo.commit("partial.txt", b"1\n2\n", "base");
        repo.commit("excluded.txt", b"1\n", "base");
        repo.commit("deleted-excluded.txt", b"1\n", "base");
        repo.commit("deleted.txt", b"1\n", "base");
        write(&repo, "partial.txt", b"1\n2\n3\n4\n");
        write(&repo, "whole.txt", b"1\n");
        stage(&repo, "excluded.txt", b"staged\n");
        write(&repo, "excluded.txt", b"on disk\n");
        repo.git(&["rm", "-q", "deleted-excluded.txt"]);
        fs::remove_file(repo.path().join("deleted.txt")).unwrap();
        let log = std::env::var_os("RAMIFY_GIT_LOG").unwrap();
        fs::write(&log, "").unwrap();

        commit(
            repo.path(),
            &CommitRequest {
                added: strings(&["partial.txt", "whole.txt"]),
                removed: strings(&["deleted.txt"]),
                partial: vec![PartialFile {
                    path: "partial.txt".to_string(),
                    content: b"1\n2\n3\n".to_vec(),
                }],
                message: "partial with excluded stage".to_string(),
                options: CommitOptions {
                    sign_off: true,
                    ..Default::default()
                },
            },
        )
        .unwrap();
    }

    // Stage that only exists in the index: different from HEAD and from the disk.
    fn repo_with_own_stage() -> TempRepo {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n2\n", "one");
        stage(&repo, "a.txt", b"1\n2\nONLY-IN-STAGE\n");
        write(&repo, "a.txt", b"1\n2\n3\n4\n");
        repo
    }

    // Merge without conflicts still to be committed: `a.txt` comes from the other branch and `b.txt` from this one.
    fn repo_with_resolved_merge() -> TempRepo {
        let repo = TempRepo::new();
        repo.commit("base.txt", b"1\n", "one");
        repo.git(&["checkout", "-q", "-b", "other"]);
        repo.commit("a.txt", b"from the other\n", "other a");
        repo.commit("c.txt", b"from the other\n", "other c");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("b.txt", b"from main\n", "main");
        repo.git(&["merge", "-q", "--no-commit", "--no-ff", "other"]);
        repo
    }

    #[test]
    fn a_merge_with_a_file_left_out_is_not_committed() {
        let repo = repo_with_resolved_merge();
        let dir = TempDir::new().unwrap();

        let error = assert_untouched(&repo, || {
            super::commit(repo.path(), dir.path(), &request(&["a.txt"]))
        });

        assert_eq!(error, CommitError::PartialMerge);
        assert!(repo.path().join(".git/MERGE_HEAD").exists());
        assert!(!dir.path().join(JOURNAL_FILE).exists());
    }

    #[test]
    fn a_merge_with_chosen_lines_is_not_committed() {
        let repo = repo_with_resolved_merge();
        let dir = TempDir::new().unwrap();
        let mut request = request(&["c.txt"]);
        request.partial = vec![partial("a.txt", b"parte\n")];

        let error = assert_untouched(&repo, || super::commit(repo.path(), dir.path(), &request));

        assert_eq!(error, CommitError::PartialMerge);
        assert!(snapshots::list(dir.path()).is_empty());
    }

    #[test]
    fn a_whole_merge_is_committed_with_its_two_parents() {
        let repo = repo_with_resolved_merge();

        commit(repo.path(), &request(&["a.txt", "c.txt"])).unwrap();

        let parents = repo.git(&["rev-list", "--parents", "-1", "HEAD"]);
        assert_eq!(parents.split(' ').count(), 3);
        assert_eq!(at_head(&repo, "a.txt"), "from the other\n");
        assert_eq!(at_head(&repo, "c.txt"), "from the other\n");
        assert_eq!(status(&repo), "");
    }

    // A changed file that the merge does not touch is not part of it: it does not make the commit partial.
    #[test]
    fn an_unrelated_unstaged_change_does_not_prevent_committing_the_merge() {
        let repo = repo_with_resolved_merge();
        write(&repo, "base.txt", b"unstaged\n");

        commit(repo.path(), &request(&["a.txt", "c.txt"])).unwrap();

        assert_eq!(status(&repo), " M base.txt\n");
    }

    #[test]
    fn with_the_index_locked_from_the_start_it_says_nothing_was_touched() {
        let repo = TempRepo::new();
        repo.commit("f.txt", b"1\n", "one");
        repo.commit("g.txt", b"1\n", "two");
        write(&repo, "f.txt", b"2\n");
        stage(&repo, "excluded.txt", b"staged\n");
        let dir = TempDir::new().unwrap();
        fs::write(repo.path().join(".git/index.lock"), b"").unwrap();

        let error = assert_untouched(&repo, || {
            super::commit(repo.path(), dir.path(), &request(&["f.txt"]))
        });

        fs::remove_file(repo.path().join(".git/index.lock")).unwrap();
        assert_eq!(error, CommitError::IndexBusy);
        let message = error.to_string();
        assert!(message.contains("nothing was touched"), "{message}");
        assert!(
            !message.contains("Could not leave the stage as it was"),
            "{message}"
        );
        assert!(dir.path().join(JOURNAL_FILE).exists());
    }

    // The retry reads the journal the first one left, and nothing it points to needs to be given back.
    #[test]
    fn repeating_with_the_index_locked_does_not_talk_of_damage_either() {
        let repo = TempRepo::new();
        repo.commit("f.txt", b"1\n", "one");
        write(&repo, "f.txt", b"2\n");
        stage(&repo, "excluded.txt", b"staged\n");
        let dir = TempDir::new().unwrap();
        fs::write(repo.path().join(".git/index.lock"), b"").unwrap();

        super::commit(repo.path(), dir.path(), &request(&["f.txt"])).unwrap_err();
        let retry = super::commit(repo.path(), dir.path(), &request(&["f.txt"])).unwrap_err();

        assert_eq!(retry, CommitError::IndexBusy);
        fs::remove_file(repo.path().join(".git/index.lock")).unwrap();
        super::commit(repo.path(), dir.path(), &request(&["f.txt"])).unwrap();
        assert_eq!(at_head(&repo, "f.txt"), "2\n");
        assert_eq!(status(&repo), "A  excluded.txt\n");
        assert!(!dir.path().join(JOURNAL_FILE).exists());
    }

    // The lock arrives with part of the index already changed: here it does have to be given back and reported.
    #[test]
    fn with_the_index_locked_midway_the_stage_is_warned_about() {
        let repo = TempRepo::new();
        repo.commit("f.txt", b"1\n", "one");
        write(&repo, "f.txt", b"2\n");
        stage(&repo, "excluded.txt", b"staged\n");
        hook(&repo, "pre-commit", "touch .git/index.lock\nexit 0");
        let dir = TempDir::new().unwrap();

        let error = super::commit(repo.path(), dir.path(), &request(&["f.txt"])).unwrap_err();

        fs::remove_file(repo.path().join(".git/index.lock")).unwrap();
        assert!(matches!(error, CommitError::Restore { .. }), "{error:?}");
        assert!(dir.path().join(JOURNAL_FILE).exists());
    }

    #[test]
    fn whole_keeps_the_stage_in_a_copy() {
        let repo = repo_with_own_stage();
        let dir = TempDir::new().unwrap();

        super::commit(repo.path(), dir.path(), &request(&["a.txt"])).unwrap();

        assert_eq!(at_head(&repo, "a.txt"), "1\n2\n3\n4\n");
        let copies = snapshots::list(dir.path());
        assert_eq!(copies.len(), 1);
        assert_eq!(copies[0].reason, "before commit");
        let blob = snapshots::stored(dir.path(), &copies[0].id, "a.txt", true).unwrap();
        assert_eq!(fs::read(blob).unwrap(), b"1\n2\nONLY-IN-STAGE\n");
    }

    #[test]
    fn whole_without_own_stage_creates_no_copy() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        stage(&repo, "a.txt", b"2\n");
        let dir = TempDir::new().unwrap();

        super::commit(repo.path(), dir.path(), &request(&["a.txt"])).unwrap();

        assert!(snapshots::list(dir.path()).is_empty());
    }

    #[test]
    fn partial_keeps_the_stage_in_a_copy() {
        let repo = repo_with_own_stage();
        let dir = TempDir::new().unwrap();
        let request = partial_request(partial("a.txt", b"1\n2\n3\n"));

        super::commit(repo.path(), dir.path(), &request).unwrap();

        assert_eq!(at_head(&repo, "a.txt"), "1\n2\n3\n");
        assert_eq!(repo.git(&["show", ":a.txt"]), "1\n2\n3\n");
        let copies = snapshots::list(dir.path());
        assert_eq!(copies.len(), 1);
        assert_eq!(copies[0].reason, "before commit");
        assert!(copies[0].entries[0].staged_blob);
        let blob = snapshots::stored(dir.path(), &copies[0].id, "a.txt", true).unwrap();
        assert_eq!(fs::read(blob).unwrap(), b"1\n2\nONLY-IN-STAGE\n");
    }

    #[test]
    fn partial_without_own_stage_creates_no_copy() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        repo.commit("b.txt", b"1\n", "two");
        write(&repo, "a.txt", b"1\n2\n3\n");
        stage(&repo, "b.txt", b"same as on disk\n");
        let dir = TempDir::new().unwrap();
        let mut request = partial_request(partial("a.txt", b"1\n2\n"));
        request.partial.push(partial("b.txt", b"partial\n"));

        super::commit(repo.path(), dir.path(), &request).unwrap();

        assert!(snapshots::list(dir.path()).is_empty());
    }

    #[test]
    fn without_a_copy_of_the_stage_there_is_no_commit() {
        let repo = repo_with_own_stage();
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("snapshots"), b"obstacle").unwrap();
        let request = partial_request(partial("a.txt", b"1\n2\n3\n"));

        let error = assert_untouched(&repo, || super::commit(repo.path(), dir.path(), &request));

        assert!(matches!(error, CommitError::Snapshot(_)), "{error:?}");
    }

    #[test]
    fn partial_nfd_keeps_the_stage_in_a_copy() {
        let repo = TempRepo::new();
        repo.git(&["config", "core.precomposeunicode", "true"]);
        write(&repo, NFD_NAME, b"1\n");
        repo.git(&["-c", "core.precomposeunicode=false", "add", "--", NFD_NAME]);
        repo.git(&["commit", "-q", "-m", "one"]);
        write(&repo, NFD_NAME, b"only in stage\n");
        repo.git(&["-c", "core.precomposeunicode=false", "add", "--", NFD_NAME]);
        write(&repo, NFD_NAME, b"1\n2\n3\n");
        let dir = TempDir::new().unwrap();
        let request = partial_request(partial(NFD_NAME, b"1\n2\n"));

        super::commit(repo.path(), dir.path(), &request).unwrap();

        let copies = snapshots::list(dir.path());
        assert_eq!(copies.len(), 1);
        let blob = snapshots::stored(dir.path(), &copies[0].id, NFD_NAME, true).unwrap();
        assert_eq!(fs::read(blob).unwrap(), b"only in stage\n");
    }

    fn tree_entries(repo: &TempRepo) -> Vec<String> {
        let output = crate::test_support::git_command()
            .current_dir(repo.path())
            .args(["ls-tree", "-r", "-z", "--name-only", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8(output.stdout)
            .unwrap()
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(str::to_string)
            .collect()
    }

    // With macOS's default value, `add` converted the path to NFC and created a second entry.
    #[test]
    fn versioned_and_modified_nfd_path() {
        let repo = TempRepo::new();
        repo.git(&["config", "core.precomposeunicode", "true"]);
        write(&repo, NFD_NAME, b"old\n");
        repo.git(&["-c", "core.precomposeunicode=false", "add", "--", NFD_NAME]);
        repo.git(&["commit", "-q", "-m", "one"]);
        write(&repo, NFD_NAME, b"new\n");

        commit(repo.path(), &request(&[NFD_NAME])).unwrap();

        assert_eq!(tree_entries(&repo), [NFD_NAME]);
        let blob = repo.git(&[
            "-c",
            "core.precomposeunicode=false",
            "show",
            &format!("HEAD:{NFD_NAME}"),
        ]);
        assert_eq!(blob, "new\n");
    }

    #[test]
    fn path_versioned_in_nfc_with_nfd_name_on_disk() {
        let repo = TempRepo::new();
        repo.git(&["config", "core.precomposeunicode", "true"]);
        write(&repo, NFD_NAME, b"old\n");
        repo.git(&["add", "--", NFD_NAME]);
        repo.git(&["commit", "-q", "-m", "one"]);
        let names = index_paths(&repo);
        assert_ne!(names[0], NFD_NAME);
        write(&repo, NFD_NAME, b"new\n");

        commit(repo.path(), &request(&[&names[0]])).unwrap();

        assert_eq!(tree_entries(&repo), names);
        assert_eq!(repo.git(&["show", &format!("HEAD:{}", names[0])]), "new\n");
        assert_eq!(status(&repo), "");
    }

    #[test]
    fn versioned_path_deleted_on_disk_selected_as_added() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        repo.commit("b.txt", b"1\n", "two");
        fs::remove_file(repo.path().join("b.txt")).unwrap();

        commit(repo.path(), &request(&["b.txt"])).unwrap();

        assert_eq!(head_files(&repo), "a.txt\n");
    }

    // App quit during a long hook: what was selected stays the same, what was excluded is left out.
    fn crash_after_unstaging(repo: &TempRepo, dir: &Path, request: &CommitRequest) -> Plan {
        let plan = plan(repo.path(), request).unwrap();
        write_journal(dir, &plan).unwrap();
        unstage(repo.path(), plan.head_exists, &plan.excluded).unwrap();
        plan
    }

    #[test]
    fn quit_during_the_commit_is_recovered() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        repo.commit("deleted.txt", b"1\n", "two");
        repo.commit("b.txt", b"1\n", "three");
        stage(&repo, "b.txt", b"staged\n");
        write(&repo, "b.txt", b"on disk\n");
        stage(&repo, "new.txt", b"new\n");
        repo.git(&["rm", "-q", "deleted.txt"]);
        write(&repo, "a.txt", b"2\n");
        let holder = TempDir::new().unwrap();
        let dir = holder.path().join("not/yet/exists");

        let recovered = assert_same_stage(&repo, || {
            let request = request(&["a.txt"]);
            crash_after_unstaging(&repo, &dir, &request);
            let state = " M a.txt\n M b.txt\n D deleted.txt\n?? new.txt\n";
            assert_eq!(status(&repo), state);
            recover(repo.path(), &dir).unwrap()
        });

        let expected = strings(&["b.txt", "deleted.txt", "new.txt"]);
        assert_eq!(recovered.restored, expected);
        assert!(recovered.skipped.is_empty());
        assert!(!dir.join(JOURNAL_FILE).exists());
        let again = recover(repo.path(), &dir).unwrap();
        assert_eq!(again, RecoveryReport::default());
    }

    #[test]
    fn quit_during_the_first_commit_is_recovered() {
        let repo = TempRepo::new();
        stage(&repo, "excluded.txt", b"staged\n");
        write(&repo, "excluded.txt", b"on disk\n");
        write(&repo, "a.txt", b"1\n");
        let dir = TempDir::new().unwrap();

        assert_same_stage(&repo, || {
            crash_after_unstaging(&repo, dir.path(), &request(&["a.txt"]));
            recover(repo.path(), dir.path()).unwrap()
        });
    }

    #[test]
    fn quit_after_the_commit_does_not_undo_it() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        repo.commit("b.txt", b"1\n", "two");
        stage(&repo, "b.txt", b"staged\n");
        write(&repo, "a.txt", b"2\n");
        let dir = TempDir::new().unwrap();
        let request = request(&["a.txt"]);

        let plan = crash_after_unstaging(&repo, dir.path(), &request);
        stage_selection(repo.path(), &request, &plan.saved).unwrap();
        let lock = commit_lock(dir.path()).unwrap();
        run_commit(repo.path(), &request, &lock).unwrap();
        drop(lock);
        let head = repo.git(&["rev-parse", "HEAD"]);

        recover(repo.path(), dir.path()).unwrap();

        assert_eq!(repo.git(&["rev-parse", "HEAD"]), head);
        assert_eq!(at_head(&repo, "a.txt"), "2\n");
        assert_eq!(status(&repo), "M  b.txt\n");
        assert_eq!(repo.git(&["show", ":b.txt"]), "staged\n");
    }

    #[test]
    fn commit_finishes_what_was_left_half_done() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        repo.commit("b.txt", b"1\n", "two");
        stage(&repo, "b.txt", b"staged\n");
        write(&repo, "a.txt", b"2\n");
        let dir = TempDir::new().unwrap();
        let request = request(&["a.txt"]);
        crash_after_unstaging(&repo, dir.path(), &request);

        super::commit(repo.path(), dir.path(), &request).unwrap();

        assert_eq!(at_head(&repo, "b.txt"), "1\n");
        assert_eq!(status(&repo), "M  b.txt\n");
        assert!(!dir.path().join(JOURNAL_FILE).exists());
    }

    #[test]
    fn unreadable_journal_is_not_deleted_nor_allows_committing() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        write(&repo, "a.txt", b"2\n");
        let dir = TempDir::new().unwrap();
        let journal = dir.path().join(JOURNAL_FILE);
        fs::write(&journal, b"not json").unwrap();

        let recovered = recover(repo.path(), dir.path()).unwrap_err();
        let error = assert_untouched(&repo, || {
            super::commit(repo.path(), dir.path(), &request(&["a.txt"]))
        });

        assert!(
            matches!(recovered, CommitError::UnreadableJournal(_)),
            "{recovered:?}"
        );
        assert!(
            matches!(error, CommitError::UnreadableJournal(_)),
            "{error:?}"
        );
        assert_eq!(fs::read(journal).unwrap(), b"not json");
    }

    #[test]
    fn without_a_journal_there_is_nothing_to_recover() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();

        assert_eq!(
            recover(repo.path(), dir.path()),
            Ok(RecoveryReport::default())
        );
    }

    #[test]
    fn the_journal_is_deleted_when_finished() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        stage(&repo, "excluded.txt", b"staged\n");
        write(&repo, "a.txt", b"2\n");
        let dir = TempDir::new().unwrap();
        hook(&repo, "pre-commit", "exit 1");

        super::commit(repo.path(), dir.path(), &request(&["a.txt"])).unwrap_err();
        assert!(!dir.path().join(JOURNAL_FILE).exists());

        fs::remove_file(repo.path().join(".git/hooks/pre-commit")).unwrap();
        super::commit(repo.path(), dir.path(), &request(&["a.txt"])).unwrap();
        assert!(!dir.path().join(JOURNAL_FILE).exists());
    }

    #[test]
    fn the_journal_exists_while_the_hook_runs() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        stage(&repo, "excluded.txt", b"staged\n");
        write(&repo, "a.txt", b"2\n");
        let dir = TempDir::new().unwrap();
        let journal = dir.path().join(JOURNAL_FILE);
        let holder = TempDir::new().unwrap();
        let copy = holder.path().join("journal");
        let script = format!("cp '{}' '{}'", journal.display(), copy.display());
        hook(&repo, "pre-commit", &script);

        super::commit(repo.path(), dir.path(), &request(&["a.txt"])).unwrap();

        let contents = fs::read_to_string(copy).unwrap();
        assert!(contents.contains("excluded.txt"), "{contents}");
        assert!(!contents.contains("a.txt"), "{contents}");
    }

    #[test]
    fn if_it_cannot_restore_the_journal_is_kept() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        hook(&repo, "post-commit", "touch .git/index.lock");
        stage(&repo, "excluded.txt", b"staged\n");
        write(&repo, "a.txt", b"2\n");
        let dir = TempDir::new().unwrap();

        let error = super::commit(repo.path(), dir.path(), &request(&["a.txt"])).unwrap_err();
        assert!(matches!(error, CommitError::Restore { .. }), "{error:?}");
        assert!(dir.path().join(JOURNAL_FILE).exists());
        let retry = recover(repo.path(), dir.path()).unwrap_err();
        assert!(matches!(retry, CommitError::Restore { .. }), "{retry:?}");
        assert!(dir.path().join(JOURNAL_FILE).exists());

        fs::remove_file(repo.path().join(".git/index.lock")).unwrap();
        let recovered = recover(repo.path(), dir.path()).unwrap();

        assert_eq!(recovered.restored, strings(&["excluded.txt"]));
        assert_eq!(status(&repo), "A  excluded.txt\n");
    }

    // Path by path, each attempt would wait for the lock: about 36 s with these 30.
    #[test]
    fn with_the_index_locked_it_does_not_retry_path_by_path() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        for number in 0..30 {
            stage(&repo, &format!("new-{number}.txt"), b"staged\n");
        }
        write(&repo, "a.txt", b"2\n");
        let dir = TempDir::new().unwrap();
        crash_after_unstaging(&repo, dir.path(), &request(&["a.txt"]));
        fs::write(repo.path().join(".git/index.lock"), b"").unwrap();
        let started = Instant::now();

        let error = recover(repo.path(), dir.path()).unwrap_err();

        let elapsed = started.elapsed();
        fs::remove_file(repo.path().join(".git/index.lock")).unwrap();
        assert!(matches!(error, CommitError::Restore { .. }), "{error:?}");
        assert!(elapsed < Duration::from_secs(15), "{elapsed:?}");
        assert!(dir.path().join(JOURNAL_FILE).exists());
    }

    // What changed is not lost either: the journal remains, and it is reported at the end.
    #[test]
    fn with_the_index_locked_what_was_skipped_is_reported_afterwards() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        repo.commit("b.txt", b"1\n", "two");
        repo.commit("c.txt", b"1\n", "three");
        stage(&repo, "b.txt", b"staged\n");
        stage(&repo, "c.txt", b"also in stage\n");
        write(&repo, "a.txt", b"2\n");
        let blob = repo.git(&["rev-parse", ":b.txt"]);
        let dir = TempDir::new().unwrap();
        crash_after_unstaging(&repo, dir.path(), &request(&["a.txt"]));
        stage(&repo, "b.txt", b"from the terminal\n");
        fs::write(repo.path().join(".git/index.lock"), b"").unwrap();

        recover(repo.path(), dir.path()).unwrap_err();

        fs::remove_file(repo.path().join(".git/index.lock")).unwrap();
        assert!(dir.path().join(JOURNAL_FILE).exists());
        let report = recover(repo.path(), dir.path()).unwrap();
        assert_eq!(report.restored, strings(&["c.txt"]));
        assert_skipped_with_blob(&report, "b.txt", &blob);
    }

    fn assert_skipped_with_blob(report: &RecoveryReport, path: &str, blob: &str) {
        let entry = report.skipped.iter().find(|entry| entry.contains(path));
        let entry = entry.unwrap_or_else(|| panic!("{path}: {report:?}"));
        assert!(entry.contains("100644"), "{entry}");
        assert!(entry.contains(blob.trim()), "{entry}");
    }

    // `ls-files -- foo` gives `foo/bar`: looking only at that exact path, `foo` seems to still be absent.
    #[test]
    fn recover_skips_a_file_that_is_now_a_folder() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        stage(&repo, "foo", b"staged\n");
        write(&repo, "a.txt", b"2\n");
        let blob = repo.git(&["rev-parse", ":foo"]);
        let dir = TempDir::new().unwrap();
        crash_after_unstaging(&repo, dir.path(), &request(&["a.txt"]));
        fs::remove_file(repo.path().join("foo")).unwrap();
        stage(&repo, "foo/bar", b"from the user\n");
        let entries = index(&repo);

        let report = recover(repo.path(), dir.path()).unwrap();

        assert_eq!(index(&repo), entries);
        assert!(report.restored.is_empty(), "{report:?}");
        assert_skipped_with_blob(&report, "foo", &blob);
        assert!(!dir.path().join(JOURNAL_FILE).exists());
        super::ensure_idle(repo.path(), dir.path()).unwrap();
    }

    // The rejection from `update-index` repeats the path: `index.lock` in it is not a locked index.
    #[test]
    fn recover_skips_a_path_named_index_lock_without_a_lock() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        stage(&repo, "tools/index.lock", b"staged\n");
        stage(&repo, "good.txt", b"good\n");
        write(&repo, "a.txt", b"2\n");
        let blob = repo.git(&["rev-parse", ":tools/index.lock"]);
        let dir = TempDir::new().unwrap();
        crash_after_unstaging(&repo, dir.path(), &request(&["a.txt"]));
        fs::remove_file(repo.path().join("tools/index.lock")).unwrap();
        stage(&repo, "tools/index.lock/child", b"from the user\n");

        let report = recover(repo.path(), dir.path()).unwrap();

        assert_eq!(report.restored, strings(&["good.txt"]));
        assert_skipped_with_blob(&report, "tools/index.lock", &blob);
        assert!(index(&repo).contains("good.txt"));
        assert!(!dir.path().join(JOURNAL_FILE).exists());
    }

    #[test]
    fn recover_skips_a_folder_that_is_now_a_file() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        stage(&repo, "foo/bar", b"staged\n");
        write(&repo, "a.txt", b"2\n");
        let blob = repo.git(&["rev-parse", ":foo/bar"]);
        let dir = TempDir::new().unwrap();
        crash_after_unstaging(&repo, dir.path(), &request(&["a.txt"]));
        fs::remove_dir_all(repo.path().join("foo")).unwrap();
        stage(&repo, "foo", b"from the user\n");
        let entries = index(&repo);

        let report = recover(repo.path(), dir.path()).unwrap();

        assert_eq!(index(&repo), entries);
        assert_skipped_with_blob(&report, "foo/bar", &blob);
        assert!(!dir.path().join(JOURNAL_FILE).exists());
    }

    // `d` new and `d/f` deleted: when `d/f` changes, neither of the two can come back.
    #[test]
    fn recover_skips_both_paths_of_a_clash_when_one_changed() {
        let repo = TempRepo::new();
        repo.commit("d/f", b"1\n", "one");
        repo.commit("a.txt", b"1\n", "two");
        repo.git(&["rm", "-q", "d/f"]);
        stage(&repo, "d", b"now file\n");
        write(&repo, "a.txt", b"2\n");
        let blob = repo.git(&["rev-parse", ":d"]);
        let dir = TempDir::new().unwrap();
        crash_after_unstaging(&repo, dir.path(), &request(&["a.txt"]));
        fs::remove_file(repo.path().join("d")).unwrap();
        stage(&repo, "d/f", b"from the user\n");
        let entries = index(&repo);

        let report = recover(repo.path(), dir.path()).unwrap();

        assert_eq!(index(&repo), entries);
        assert!(report.restored.is_empty(), "{report:?}");
        assert_eq!(report.skipped.len(), 2, "{report:?}");
        assert_skipped_with_blob(&report, "\"d\"", &blob);
        assert!(report.skipped.iter().any(|entry| entry.contains("\"d/f\"")));
        assert!(!dir.path().join(JOURNAL_FILE).exists());
    }

    // The clash is resolved within the journal itself: `d/f` is removed and `d` can come back.
    #[test]
    fn recover_without_changes_restores_a_file_and_its_old_folder() {
        let repo = TempRepo::new();
        repo.commit("d/f", b"1\n", "one");
        repo.commit("a.txt", b"1\n", "two");
        repo.git(&["rm", "-q", "d/f"]);
        stage(&repo, "d", b"now file\n");
        write(&repo, "a.txt", b"2\n");
        let dir = TempDir::new().unwrap();

        let report = assert_same_stage(&repo, || {
            crash_after_unstaging(&repo, dir.path(), &request(&["a.txt"]));
            recover(repo.path(), dir.path()).unwrap()
        });

        assert_eq!(report.restored, strings(&["d", "d/f"]));
        assert!(report.skipped.is_empty(), "{report:?}");
    }

    // A single impossible path does not drag down the others in its batch.
    #[test]
    fn recover_skips_only_the_path_that_fails() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        write(&repo, "good.txt", b"staged\n");
        let blob = repo.git(&["hash-object", "-w", "good.txt"]);
        let dir = TempDir::new().unwrap();
        let entry = serde_json::json!({
            "restore": { "Entry": { "mode": "100644", "hash": blob.trim() } },
            "left": "Absent",
        });
        let journal = serde_json::json!({ "a/.git/malo": entry, "good.txt": entry });
        fs::write(dir.path().join(JOURNAL_FILE), journal.to_string()).unwrap();

        let report = recover(repo.path(), dir.path()).unwrap();

        assert_eq!(report.restored, strings(&["good.txt"]));
        assert_skipped_with_blob(&report, "a/.git/malo", &blob);
        assert_eq!(status(&repo), "A  good.txt\n");
        assert!(!dir.path().join(JOURNAL_FILE).exists());
    }

    #[test]
    fn without_a_written_journal_nothing_is_touched() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        stage(&repo, "excluded.txt", b"staged\n");
        write(&repo, "a.txt", b"2\n");
        let dir = TempDir::new().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o500)).unwrap();

        let error = assert_untouched(&repo, || {
            super::commit(repo.path(), dir.path(), &request(&["a.txt"]))
        });

        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        assert!(matches!(error, CommitError::Journal(_)), "{error:?}");
    }

    #[test]
    fn quit_before_unstaging_skips_nothing() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        repo.commit("b.txt", b"1\n", "two");
        stage(&repo, "b.txt", b"staged\n");
        write(&repo, "a.txt", b"2\n");
        let dir = TempDir::new().unwrap();
        let plan = plan(repo.path(), &request(&["a.txt"])).unwrap();
        write_journal(dir.path(), &plan).unwrap();

        let recovered = assert_same_stage(&repo, || recover(repo.path(), dir.path()).unwrap());

        assert_eq!(recovered.restored, strings(&["b.txt"]));
        assert!(recovered.skipped.is_empty());
        assert!(!dir.path().join(JOURNAL_FILE).exists());
    }

    #[test]
    fn recover_skips_a_path_that_changed() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        repo.commit("b.txt", b"1\n", "two");
        repo.commit("c.txt", b"1\n", "three");
        stage(&repo, "b.txt", b"staged\n");
        stage(&repo, "c.txt", b"also in stage\n");
        write(&repo, "a.txt", b"2\n");
        let blob = repo.git(&["rev-parse", ":b.txt"]);
        let dir = TempDir::new().unwrap();
        crash_after_unstaging(&repo, dir.path(), &request(&["a.txt"]));
        stage(&repo, "b.txt", b"from the terminal\n");
        repo.git(&["commit", "-q", "-m", "terminal", "--", "b.txt"]);

        let report = recover(repo.path(), dir.path()).unwrap();

        assert_eq!(report.restored, strings(&["c.txt"]));
        assert_eq!(report.skipped.len(), 1);
        assert!(report.skipped[0].contains("b.txt"), "{:?}", report.skipped);
        assert!(report.skipped[0].contains("100644"), "{:?}", report.skipped);
        assert!(
            report.skipped[0].contains(blob.trim()),
            "{:?}",
            report.skipped
        );
        assert_eq!(repo.git(&["cat-file", "-p", blob.trim()]), "staged\n");
        assert_eq!(status(&repo), " M a.txt\nM  c.txt\n");
        assert_eq!(repo.git(&["show", ":b.txt"]), "from the terminal\n");
        assert!(!dir.path().join(JOURNAL_FILE).exists());
    }

    // `b.txt` deleted by the other branch: stage 2 of the conflict is exactly the entry that
    // `unstage` left, so comparing the entry is not enough.
    #[test]
    fn recover_does_not_delete_a_conflict() {
        let repo = TempRepo::new();
        repo.commit("b.txt", b"base\n", "base");
        repo.git(&["checkout", "-q", "-b", "other"]);
        repo.git(&["rm", "-q", "b.txt"]);
        repo.git(&["commit", "-q", "-m", "other"]);
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("b.txt", b"main\n", "main");
        repo.commit("a.txt", b"1\n", "a");
        stage(&repo, "b.txt", b"staged\n");
        write(&repo, "a.txt", b"2\n");
        let dir = TempDir::new().unwrap();
        crash_after_unstaging(&repo, dir.path(), &request(&["a.txt"]));
        repo.git(&["checkout", "--", "b.txt", "a.txt"]);
        let merge = crate::test_support::git_command()
            .current_dir(repo.path())
            .args(["merge", "--no-edit", "other"])
            .output()
            .unwrap();
        assert!(!merge.status.success());
        let conflict = repo.git(&["ls-files", "-u"]);
        assert!(!conflict.is_empty());

        let report = recover(repo.path(), dir.path()).unwrap();

        assert_eq!(repo.git(&["ls-files", "-u"]), conflict);
        assert_eq!(report.skipped.len(), 1, "{report:?}");
        assert!(report.skipped[0].contains("b.txt"));
        assert!(report.restored.is_empty());
    }

    #[test]
    fn commit_reports_what_recover_skipped_only_once() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        repo.commit("b.txt", b"1\n", "two");
        stage(&repo, "b.txt", b"staged\n");
        write(&repo, "a.txt", b"2\n");
        let dir = TempDir::new().unwrap();
        let request = request(&["a.txt"]);
        crash_after_unstaging(&repo, dir.path(), &request);
        stage(&repo, "b.txt", b"other\n");

        let first = super::commit(repo.path(), dir.path(), &request).unwrap_err();

        let CommitError::RecoverySkipped(entries) = &first else {
            panic!("{first:?}")
        };
        assert!(entries[0].contains("b.txt"), "{entries:?}");
        super::commit(repo.path(), dir.path(), &request).unwrap();
        assert_eq!(status(&repo), "M  b.txt\n");
    }

    #[test]
    fn commit_with_another_commit_alive_touches_nothing() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        write(&repo, "a.txt", b"2\n");
        let dir = TempDir::new().unwrap();
        let _held = commit_lock(dir.path()).unwrap();

        let error = assert_untouched(&repo, || {
            super::commit(repo.path(), dir.path(), &request(&["a.txt"]))
        });

        assert_eq!(error, CommitError::CommitRunning);
    }

    #[test]
    fn the_lock_is_released_when_the_commit_finishes() {
        let repo = TempRepo::new();
        write(&repo, "a.txt", b"1\n");
        let dir = TempDir::new().unwrap();

        super::commit(repo.path(), dir.path(), &request(&["a.txt"])).unwrap();

        drop(commit_lock(dir.path()).unwrap());
        assert!(!recover(repo.path(), dir.path()).unwrap().pending);
    }

    const ORPHAN_REPO: &str = "RAMIFY_TEST_REPO";
    const ORPHAN_DIR: &str = "RAMIFY_TEST_DIR";

    // It is launched by `recover_with_a_live_commit_touches_nothing` in another process, which it then kills.
    #[test]
    #[ignore = "helper process of recover_with_a_live_commit_touches_nothing"]
    fn helper_commit_that_stays_orphaned() {
        let (Some(repo), Some(dir)) = (std::env::var_os(ORPHAN_REPO), std::env::var_os(ORPHAN_DIR))
        else {
            return;
        };

        let _ = super::commit(Path::new(&repo), Path::new(&dir), &request(&["a.txt"]));
    }

    fn wait_until(mut done: impl FnMut() -> bool, what: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done() {
            assert!(Instant::now() < deadline, "timed out: {what}");
            thread::sleep(Duration::from_millis(20));
        }
    }

    // A real quit: the app dies with `kill -9` during a long hook, and `git commit` and its hook
    // stay alive. `git commit` rereads the index after `pre-commit`, so restoring at that
    // moment would put what was excluded into a commit of `a.txt`.
    #[test]
    fn recover_with_a_live_commit_touches_nothing() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        repo.commit("b.txt", b"1\n", "two");
        stage(&repo, "b.txt", b"excluded\n");
        write(&repo, "a.txt", b"2\n");
        let holder = TempDir::new().unwrap();
        let dir = holder.path().join("data");
        let (started, release) = (
            holder.path().join("started"),
            holder.path().join("continue"),
        );
        let script = format!(
            "touch '{}'\ni=0\nwhile [ ! -f '{}' ] && [ $i -lt 400 ]; do sleep 0.05; i=$((i+1)); done",
            started.display(),
            release.display()
        );
        hook(&repo, "pre-commit", &script);
        let mut app = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "--ignored"])
            .arg("commit::tests::helper_commit_that_stays_orphaned")
            .env(ORPHAN_REPO, repo.path())
            .env(ORPHAN_DIR, &dir)
            .env("TMPDIR", holder.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        wait_until(|| started.exists(), "the hook did not start");
        app.kill().unwrap();
        app.wait().unwrap();
        assert!(dir.join(JOURNAL_FILE).exists());

        let report = assert_same_stage(&repo, || recover(repo.path(), &dir).unwrap());

        assert_eq!(
            report,
            RecoveryReport {
                pending: true,
                ..Default::default()
            }
        );
        assert!(dir.join(JOURNAL_FILE).exists());
        let blocked = super::commit(repo.path(), &dir, &request(&["a.txt"])).unwrap_err();
        assert_eq!(blocked, CommitError::CommitRunning);

        fs::write(&release, b"").unwrap();
        wait_until(
            || !recover(repo.path(), &dir).unwrap().pending,
            "the orphan commit did not finish",
        );

        assert_eq!(at_head(&repo, "a.txt"), "2\n");
        assert_eq!(at_head(&repo, "b.txt"), "1\n");
        assert_eq!(repo.git(&["show", ":b.txt"]), "excluded\n");
        assert!(!dir.join(JOURNAL_FILE).exists());
    }

    #[test]
    fn nfd_intent_comes_back_if_the_hook_fails() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        repo.git(&["config", "core.precomposeunicode", "true"]);
        hook(&repo, "pre-commit", "exit 1");
        write(&repo, NFD_NAME, b"1\n");
        repo.git(&[
            "-c",
            "core.precomposeunicode=false",
            "add",
            "-N",
            "--",
            NFD_NAME,
        ]);
        let names = index_paths(&repo);
        assert!(names.iter().any(|name| name == NFD_NAME), "{names:?}");

        let error = commit(repo.path(), &request(&[NFD_NAME])).unwrap_err();

        assert!(matches!(error, CommitError::Hook(_)), "{error:?}");
        assert_eq!(index_paths(&repo), names);
    }

    #[test]
    fn nfc_intent_with_nfd_name_on_disk_comes_back_if_the_hook_fails() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        repo.git(&["config", "core.precomposeunicode", "true"]);
        hook(&repo, "pre-commit", "exit 1");
        write(&repo, NFD_NAME, b"1\n");
        repo.git(&["add", "-N", "--", NFD_NAME]);
        let names = index_paths(&repo);
        let name = names.iter().find(|name| *name != "a.txt").unwrap().clone();
        assert_ne!(name, NFD_NAME);

        let error = commit(repo.path(), &request(&[&name])).unwrap_err();

        assert!(matches!(error, CommitError::Hook(_)), "{error:?}");
        assert_eq!(index_paths(&repo), names);
    }

    #[test]
    fn request_from_json() {
        let json = r#"{
            "added": ["a.txt"],
            "removed": [],
            "partial": [{ "path": "b.txt", "content": [104, 101, 108, 108, 111] }],
            "message": "hello",
            "options": {
                "amend": false,
                "signOff": true,
                "skipHooks": true,
                "author": null,
                "cleanup": false
            }
        }"#;

        let request: CommitRequest = serde_json::from_str(json).unwrap();

        assert_eq!(request.partial[0].content, b"hello");
        assert!(request.options.sign_off && request.options.skip_hooks);
    }

    // Signing.

    // A `gpg` that records its arguments and fails, or signs if `sign` says so.
    fn fake_gpg(repo: &TempRepo, dir: &Path, sign: bool) -> std::path::PathBuf {
        let log = dir.join("gpg-args");
        let script = dir.join("fake-gpg");
        let body = if sign {
            "echo '[GNUPG:] SIG_CREATED ' >&2\necho signature"
        } else {
            "exit 1"
        };
        fs::write(
            &script,
            format!("#!/bin/sh\necho \"$*\" >> '{}'\n{body}\n", log.display()),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        repo.git(&["config", "gpg.program", script.to_str().unwrap()]);
        log
    }

    // Decision 1: git signs by itself from `commit.gpgSign`, as in 241; Ramify never passes `-S`.
    #[test]
    fn signing_comes_from_the_config_and_its_key() {
        let repo = TempRepo::new();
        let tools = TempDir::new().unwrap();
        repo.commit("a.txt", b"1\n", "one");
        write(&repo, "a.txt", b"2\n");
        let log = fake_gpg(&repo, tools.path(), true);
        repo.git(&["config", "commit.gpgSign", "true"]);
        repo.git(&["config", "user.signingkey", "ABC123"]);

        commit(repo.path(), &request(&["a.txt"])).unwrap();

        assert!(fs::read_to_string(log).unwrap().contains("ABC123"));
        assert_eq!(commits(&repo), "2\n");
    }

    #[test]
    fn a_commit_is_signed_by_git_from_the_config_without_dash_s() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        repo.sign_commits();
        write(&repo, "a.txt", b"2\n");

        commit(repo.path(), &request(&["a.txt"])).unwrap();

        assert!(repo.git(&["cat-file", "commit", "HEAD"]).contains("gpgsig "));

        // Nothing Ramify passes signs. With the config off and every option on, a recording gpg is
        // never called and the commit is unsigned: a `-S` or `--gpg-sign` would make git run it.
        let tools = TempDir::new().unwrap();
        repo.git(&["config", "--unset", "gpg.format"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        let log = fake_gpg(&repo, tools.path(), true);
        write(&repo, "a.txt", b"3\n");
        let mut everything = request(&["a.txt"]);
        everything.options = CommitOptions {
            amend: true,
            sign_off: true,
            skip_hooks: true,
            author: Some("A <a@example.com>".to_string()),
            cleanup: true,
        };

        commit(repo.path(), &everything).unwrap();

        assert!(!log.exists(), "{:?}", fs::read_to_string(&log));
        assert!(!repo.git(&["cat-file", "commit", "HEAD"]).contains("gpgsig "));

        // The same gpg is git's own call once the config asks for it.
        repo.git(&["config", "commit.gpgsign", "true"]);
        write(&repo, "a.txt", b"4\n");
        commit(repo.path(), &request(&["a.txt"])).unwrap();
        assert!(fs::read_to_string(&log).unwrap().contains("-bsau"));
    }

    #[test]
    fn without_signing_gpg_is_not_called() {
        let repo = TempRepo::new();
        let tools = TempDir::new().unwrap();
        repo.commit("a.txt", b"1\n", "one");
        write(&repo, "a.txt", b"2\n");
        let log = fake_gpg(&repo, tools.path(), false);

        commit(repo.path(), &request(&["a.txt"])).unwrap();

        assert!(!log.exists());
    }

    // Commit of the index as it is.

    fn staged(repo: &Path, options: &CommitOptions) -> Result<String, CommitError> {
        let dir = TempDir::new().unwrap();
        commit_staged(repo, dir.path(), options, "from the stage")
    }

    #[test]
    fn stage_commit_uses_the_index_as_is() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        repo.commit("b.txt", b"1\n", "two");
        stage(&repo, "a.txt", b"staged\n");
        write(&repo, "a.txt", b"on disk\n");
        write(&repo, "b.txt", b"unstaged\n");

        let hash = staged(repo.path(), &CommitOptions::default()).unwrap();

        assert_eq!(repo.git(&["rev-parse", "HEAD"]).trim(), hash);
        assert_eq!(at_head(&repo, "a.txt"), "staged\n");
        assert_eq!(at_head(&repo, "b.txt"), "1\n");
        assert_eq!(body(&repo).trim(), "from the stage");
        assert_eq!(status(&repo), " M a.txt\n M b.txt\n");
    }

    #[test]
    fn stage_commit_with_nothing_staged() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        write(&repo, "a.txt", b"2\n");

        let error = staged(repo.path(), &CommitOptions::default()).unwrap_err();

        assert_eq!(error, CommitError::NothingToCommit);
    }

    #[test]
    fn stage_commit_with_conflicts() {
        let repo = TempRepo::new();
        repo.commit("f.txt", b"base\n", "base");
        repo.git(&["checkout", "-q", "-b", "other"]);
        repo.commit("f.txt", b"other\n", "other");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("f.txt", b"main\n", "main");
        let _ = crate::test_support::git_command()
            .current_dir(repo.path())
            .args(["merge", "--no-edit", "other"])
            .output()
            .unwrap();

        let error = staged(repo.path(), &CommitOptions::default()).unwrap_err();

        assert_eq!(error, CommitError::Unmerged);
    }

    #[test]
    fn stage_commit_respects_the_options() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        hook(&repo, "pre-commit", "exit 1");
        stage(&repo, "a.txt", b"2\n");
        let options = CommitOptions {
            amend: true,
            skip_hooks: true,
            sign_off: true,
            ..Default::default()
        };

        staged(repo.path(), &options).unwrap();

        assert_eq!(commits(&repo), "1\n");
        assert_eq!(at_head(&repo, "a.txt"), "2\n");
        assert!(body(&repo).contains("Signed-off-by"));
    }

    #[test]
    fn stage_commit_first_recovers_an_interrupted_commit() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1\n", "one");
        repo.commit("b.txt", b"1\n", "two");
        stage(&repo, "b.txt", b"excluded\n");
        let hash = repo.git(&["rev-parse", ":b.txt"]).trim().to_string();
        repo.git(&["reset", "-q", "HEAD", "--", "b.txt"]);
        let head = repo.git(&["rev-parse", "HEAD:b.txt"]).trim().to_string();
        let entry = |hash: &str| Saved::Entry {
            mode: DEFAULT_MODE.to_string(),
            hash: hash.to_string(),
        };
        let journal = BTreeMap::from([(
            "b.txt".to_string(),
            JournalEntry {
                restore: entry(&hash),
                left: entry(&head),
            },
        )]);
        fs::write(
            dir.path().join(JOURNAL_FILE),
            serde_json::to_vec(&journal).unwrap(),
        )
        .unwrap();

        commit_staged(repo.path(), dir.path(), &CommitOptions::default(), "with b").unwrap();

        assert_eq!(at_head(&repo, "b.txt"), "excluded\n");
        assert!(!dir.path().join(JOURNAL_FILE).exists());
    }

    #[test]
    fn stage_commit_takes_the_lock() {
        let repo = TempRepo::new();
        let dir = TempDir::new().unwrap();
        repo.commit("a.txt", b"1\n", "one");
        stage(&repo, "a.txt", b"2\n");
        let held = commit_lock(dir.path()).unwrap();

        let error =
            commit_staged(repo.path(), dir.path(), &CommitOptions::default(), "x").unwrap_err();

        assert_eq!(error, CommitError::CommitRunning);
        drop(held);
    }

    // Avisos.

    fn all_warnings() -> Settings {
        Settings::default()
    }

    // Without this the global `core.autocrlf` of whoever runs the tests applies.
    fn crlf_repo(autocrlf: &str) -> TempRepo {
        let repo = TempRepo::new();
        repo.git(&["config", "core.autocrlf", autocrlf]);
        repo.commit("base.txt", b"base\n", "base");
        repo
    }

    #[test]
    fn crlf_notice() {
        let repo = crlf_repo("false");
        write(&repo, "a.txt", b"one\r\ntwo\r\n");
        write(&repo, "b.txt", b"one\ntwo\n");

        let found = warnings(repo.path(), &request(&["a.txt", "b.txt"]), &all_warnings());

        assert_eq!(found, vec![CommitWarning::Crlf(strings(&["a.txt"]))]);
    }

    #[test]
    fn no_warning_with_autocrlf_input_or_true() {
        for value in ["input", "true", "TRUE"] {
            let repo = crlf_repo(value);
            write(&repo, "a.txt", b"one\r\ntwo\r\n");

            let found = warnings(repo.path(), &request(&["a.txt"]), &all_warnings());

            assert_eq!(found, vec![], "core.autocrlf={value}");
        }
    }

    #[test]
    fn no_warning_with_autocrlf_input_for_a_file_that_already_had_crlf() {
        let repo = crlf_repo("false");
        repo.commit("a.txt", b"one\r\n", "crlf");
        repo.git(&["config", "core.autocrlf", "input"]);
        write(&repo, "a.txt", b"one\r\ntwo\r\n");

        let found = warnings(repo.path(), &request(&["a.txt"]), &all_warnings());

        assert_eq!(found, vec![]);
    }

    #[test]
    fn no_warning_with_text_attributes() {
        for attributes in ["*.txt text", "*.txt -text", "*.txt crlf", "*.txt text=auto"] {
            let repo = crlf_repo("false");
            fs::write(repo.path().join(".gitattributes"), attributes).unwrap();
            write(&repo, "a.txt", b"one\r\ntwo\r\n");

            let found = warnings(repo.path(), &request(&["a.txt"]), &all_warnings());

            assert_eq!(found, vec![], "{attributes}");
        }
    }

    #[test]
    fn warns_only_if_crlf_would_be_stored() {
        let repo = crlf_repo("false");
        // `eol` is not one of the attributes IntelliJ looks at, but git will store LF.
        fs::write(repo.path().join(".gitattributes"), "*.txt eol=lf\n").unwrap();
        write(&repo, "a.txt", b"one\r\ntwo\r\n");

        let found = warnings(repo.path(), &request(&["a.txt"]), &all_warnings());

        assert_eq!(found, vec![]);
    }

    #[test]
    fn binaries_and_deleted_files_do_not_warn() {
        let repo = crlf_repo("false");
        write(&repo, "a.bin", b"one\0\r\n");
        let mut sent = request(&["a.bin", "missing.txt"]);
        sent.removed = strings(&["base.txt"]);

        assert_eq!(warnings(repo.path(), &sent, &all_warnings()), vec![]);
    }

    #[test]
    fn partial_content_is_checked_as_it_would_be_stored() {
        let repo = crlf_repo("false");
        write(&repo, "a.txt", b"one\r\ntwo\r\n");
        write(&repo, "b.txt", b"one\r\n");
        let mut sent = partial_request(partial("a.txt", b"one\r\n"));
        sent.partial.push(partial("b.txt", b"one\n"));

        let found = warnings(repo.path(), &sent, &all_warnings());

        assert_eq!(found, vec![CommitWarning::Crlf(strings(&["a.txt"]))]);
    }

    #[test]
    fn crlf_warning_disabled() {
        let repo = crlf_repo("false");
        write(&repo, "a.txt", b"one\r\n");
        let settings = Settings {
            warn_crlf: false,
            ..Default::default()
        };

        assert_eq!(
            warnings(repo.path(), &request(&["a.txt"]), &settings),
            vec![]
        );
    }

    #[test]
    fn in_stage_mode_what_is_in_the_index_is_checked() {
        let repo = crlf_repo("false");
        stage(&repo, "a.txt", b"one\r\n");
        write(&repo, "a.txt", b"one\n");
        stage(&repo, "b.txt", b"one\n");
        write(&repo, "b.txt", b"one\r\n");
        write(&repo, "c.txt", b"unstaged\r\n");
        repo.git(&["rm", "-q", "--cached", "base.txt"]);

        let found = staged_warnings(repo.path(), &all_warnings());

        assert_eq!(found, vec![CommitWarning::Crlf(strings(&["a.txt"]))]);
    }

    #[test]
    fn detached_head_notice() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        repo.git(&["checkout", "-q", "--detach"]);

        let found = warnings(repo.path(), &CommitRequest::default(), &all_warnings());

        assert_eq!(found, vec![CommitWarning::DetachedHead]);
    }

    #[test]
    fn no_warning_on_a_branch_or_without_commits() {
        let repo = TempRepo::new();
        assert_eq!(
            warnings(repo.path(), &CommitRequest::default(), &all_warnings()),
            vec![]
        );
        repo.commit("a.txt", b"1\n", "one");
        assert_eq!(
            warnings(repo.path(), &CommitRequest::default(), &all_warnings()),
            vec![]
        );
    }

    fn rebase_conflict(repo: &TempRepo, args: &[&str]) {
        repo.commit("a.txt", b"base\n", "base");
        repo.git(&["checkout", "-q", "-b", "other"]);
        repo.commit("a.txt", b"other\n", "other");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("a.txt", b"main\n", "main");
        let rebase = crate::test_support::git_command()
            .current_dir(repo.path())
            .args(args)
            .output()
            .unwrap();
        assert!(!rebase.status.success());
    }

    #[test]
    fn warning_during_rebase() {
        let repo = TempRepo::new();
        rebase_conflict(&repo, &["rebase", "--apply", "other"]);

        let found = warnings(repo.path(), &CommitRequest::default(), &all_warnings());

        assert_eq!(found, vec![CommitWarning::Rebasing]);
        repo.git(&["rebase", "--abort"]);
    }

    // `GitDetachedRootCheckinHandler` does not warn if the rebase has the `interactive` file, and git
    // writes it in every rebase with the `merge` engine.
    #[test]
    fn no_warning_during_an_interactive_rebase() {
        let repo = TempRepo::new();
        rebase_conflict(&repo, &["rebase", "--merge", "other"]);

        let found = warnings(repo.path(), &CommitRequest::default(), &all_warnings());

        assert_eq!(found, vec![]);
        repo.git(&["rebase", "--abort"]);
    }

    #[test]
    fn no_detached_head_warning_during_a_merge() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"base\n", "base");
        repo.git(&["checkout", "-q", "-b", "other"]);
        repo.commit("b.txt", b"other\n", "other");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("a.txt", b"main\n", "main");
        repo.git(&["checkout", "-q", "--detach"]);
        repo.git(&["merge", "-q", "--no-commit", "--no-ff", "other"]);

        let found = warnings(repo.path(), &CommitRequest::default(), &all_warnings());

        assert_eq!(found, vec![]);
    }

    #[test]
    fn detached_head_warning_disabled() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        repo.git(&["checkout", "-q", "--detach"]);
        let settings = Settings {
            warn_detached: false,
            ..Default::default()
        };

        assert_eq!(
            warnings(repo.path(), &CommitRequest::default(), &settings),
            vec![]
        );
    }

    #[test]
    fn warnings_are_sent_to_the_interface_with_their_type() {
        assert_eq!(
            serde_json::to_value(CommitWarning::Crlf(strings(&["a.txt"]))).unwrap(),
            serde_json::json!({ "kind": "crlf", "paths": ["a.txt"] })
        );
        assert_eq!(
            serde_json::to_value(CommitWarning::DetachedHead).unwrap(),
            serde_json::json!({ "kind": "detachedHead" })
        );
        assert_eq!(
            serde_json::to_value(CommitWarning::Rebasing).unwrap(),
            serde_json::json!({ "kind": "rebasing" })
        );
        assert_eq!(
            serde_json::to_value(CommitWarning::LargeFiles(vec![("a.bin".to_string(), 7)]))
                .unwrap(),
            serde_json::json!({ "kind": "largeFiles", "paths": [["a.bin", 7]] })
        );
        assert_eq!(
            serde_json::to_value(CommitWarning::BadFileNames(vec![BadFileName {
                name: "a:b".to_string(),
                problem: BadNameProblem::InvalidChar,
            }]))
            .unwrap(),
            serde_json::json!({
                "kind": "badFileNames",
                "paths": [{ "name": "a:b", "problem": "invalidChar" }]
            })
        );
    }

    const MB: usize = 1024 * 1024;

    // A repository with one commit and 1 MB as the limit, so the files of the tests stay small.
    fn small_limit() -> (TempRepo, Settings) {
        let repo = crlf_repo("false");
        let settings = Settings {
            large_files_limit_mb: 1,
            ..Default::default()
        };
        (repo, settings)
    }

    fn large(path: &str, size: usize) -> CommitWarning {
        CommitWarning::LargeFiles(vec![(path.to_string(), size as u64)])
    }

    fn bad(name: &str, problem: BadNameProblem) -> CommitWarning {
        CommitWarning::BadFileNames(vec![BadFileName {
            name: name.to_string(),
            problem,
        }])
    }

    #[test]
    fn a_file_over_the_limit_warns_with_its_size() {
        let (repo, settings) = small_limit();
        write(&repo, "big.bin", &vec![b'x'; MB + 1]);
        // The limit itself is not over it.
        write(&repo, "exact.bin", &vec![b'x'; MB]);
        write(&repo, "small.txt", b"small\n");

        let sent = request(&["big.bin", "exact.bin", "small.txt", "gone.bin"]);
        let found = warnings(repo.path(), &sent, &settings);

        assert_eq!(found, vec![large("big.bin", MB + 1)]);
    }

    #[test]
    fn every_file_over_the_limit_is_listed() {
        let (repo, settings) = small_limit();
        write(&repo, "a.bin", &vec![b'x'; MB + 1]);
        write(&repo, "dir/b.bin", &vec![b'x'; 2 * MB]);

        let found = warnings(repo.path(), &request(&["a.bin", "dir/b.bin"]), &settings);

        assert_eq!(
            found,
            vec![CommitWarning::LargeFiles(vec![
                ("a.bin".to_string(), (MB + 1) as u64),
                ("dir/b.bin".to_string(), (2 * MB) as u64),
            ])]
        );
    }

    #[test]
    fn the_default_limit_is_50_mb() {
        let repo = crlf_repo("false");
        write(&repo, "a.bin", &vec![b'x'; 2 * MB]);

        let found = warnings(repo.path(), &request(&["a.bin"]), &all_warnings());

        assert_eq!(found, vec![]);
    }

    #[test]
    fn lfs_turns_off_the_size_check() {
        let (repo, settings) = small_limit();
        write(&repo, "big.bin", &vec![b'x'; 2 * MB]);
        assert_eq!(
            warnings(repo.path(), &request(&["big.bin"]), &settings),
            vec![large("big.bin", 2 * MB)]
        );

        repo.git(&["config", "lfs.repositoryformatversion", "0"]);

        assert_eq!(
            warnings(repo.path(), &request(&["big.bin"]), &settings),
            vec![]
        );
    }

    #[test]
    fn the_size_check_can_be_turned_off_or_given_no_limit() {
        let (repo, settings) = small_limit();
        write(&repo, "big.bin", &vec![b'x'; 2 * MB]);
        let sent = request(&["big.bin"]);

        let off = Settings {
            warn_large_files: false,
            ..settings.clone()
        };
        let none = Settings {
            large_files_limit_mb: 0,
            ..settings
        };

        assert_eq!(warnings(repo.path(), &sent, &off), vec![]);
        assert_eq!(warnings(repo.path(), &sent, &none), vec![]);
    }

    #[test]
    fn a_partial_file_is_measured_by_what_goes_into_the_index() {
        let (repo, settings) = small_limit();
        // Big on disk, but only a few lines of it are chosen.
        write(&repo, "a.txt", &vec![b'x'; 2 * MB]);
        let small = partial_request(partial("a.txt", b"few lines\n"));
        assert_eq!(warnings(repo.path(), &small, &settings), vec![]);

        // Small on disk, with a big content chosen for the index.
        write(&repo, "b.txt", b"small\n");
        let big = partial_request(partial("b.txt", &vec![b'x'; MB + 5]));
        assert_eq!(
            warnings(repo.path(), &big, &settings),
            vec![large("b.txt", MB + 5)]
        );
    }

    #[test]
    fn a_deleted_file_is_not_measured() {
        let (repo, settings) = small_limit();
        repo.commit("big.bin", &vec![b'x'; 2 * MB], "big");
        fs::remove_file(repo.path().join("big.bin")).unwrap();
        let sent = CommitRequest {
            removed: strings(&["big.bin"]),
            ..Default::default()
        };

        assert_eq!(warnings(repo.path(), &sent, &settings), vec![]);
    }

    #[test]
    fn in_stage_mode_the_size_of_the_staged_blob_is_checked() {
        let (repo, settings) = small_limit();
        stage(&repo, "staged.bin", &vec![b'x'; 2 * MB]);
        // Big on disk, small in the index: that is what the commit takes.
        stage(&repo, "shrunk.bin", b"small\n");
        write(&repo, "shrunk.bin", &vec![b'x'; 3 * MB]);
        write(&repo, "unstaged.bin", &vec![b'x'; 3 * MB]);

        let found = staged_warnings(repo.path(), &settings);

        assert_eq!(found, vec![large("staged.bin", 2 * MB)]);
    }

    #[test]
    fn an_added_file_named_aux_warns() {
        let repo = crlf_repo("false");
        write(&repo, "aux.txt", b"x\n");

        let found = warnings(repo.path(), &request(&["aux.txt"]), &all_warnings());

        assert_eq!(found, vec![bad("aux.txt", BadNameProblem::Reserved)]);
    }

    #[test]
    fn reserved_names_ignore_the_case_and_the_extension_and_look_at_every_folder() {
        let repo = crlf_repo("false");
        for (path, name) in [
            ("Com1.log", "Com1.log"),
            ("NUL", "NUL"),
            ("src/lpt9.tar", "lpt9.tar"),
            ("src/prn/ok.txt", "prn"),
            ("con/inner/ok.txt", "con"),
        ] {
            write(&repo, path, b"x\n");

            let found = warnings(repo.path(), &request(&[path]), &all_warnings());

            assert_eq!(found, vec![bad(name, BadNameProblem::Reserved)], "{path}");
        }
    }

    #[test]
    fn names_that_only_look_like_reserved_ones_do_not_warn() {
        let repo = crlf_repo("false");
        // Longer or shorter than the reserved ones, or the reserved word only inside a longer base name.
        for path in [
            "auxiliary.txt",
            "au.txt",
            "com0.txt",
            "com10.txt",
            "aux.tar.gz",
            ".aux",
            "xaux",
        ] {
            write(&repo, path, b"x\n");

            let found = warnings(repo.path(), &request(&[path]), &all_warnings());

            assert_eq!(found, vec![], "{path}");
        }
    }

    #[test]
    fn characters_that_windows_does_not_accept_warn() {
        let repo = crlf_repo("false");
        for name in [
            "a<b", "a>b", "a:b", "a\"b", "a\\b", "a|b", "a?b", "a*b", "a\tb",
        ] {
            write(&repo, name, b"x\n");

            let found = warnings(repo.path(), &request(&[name]), &all_warnings());

            assert_eq!(
                found,
                vec![bad(name, BadNameProblem::InvalidChar)],
                "{name:?}"
            );
        }
    }

    #[test]
    fn a_renamed_file_with_a_colon_warns() {
        let repo = crlf_repo("false");
        repo.commit("a.txt", b"one\n", "a");
        fs::rename(repo.path().join("a.txt"), repo.path().join("a:b.txt")).unwrap();
        let mut sent = request(&["a:b.txt"]);
        sent.removed = strings(&["a.txt"]);

        let found = warnings(repo.path(), &sent, &all_warnings());

        assert_eq!(found, vec![bad("a:b.txt", BadNameProblem::InvalidChar)]);
    }

    #[test]
    fn a_file_that_was_already_in_head_is_not_checked() {
        let repo = crlf_repo("false");
        repo.commit("aux.txt", b"one\n", "aux");
        repo.commit("co:lon.txt", b"one\n", "colon");
        write(&repo, "aux.txt", b"two\n");
        write(&repo, "co:lon.txt", b"two\n");

        let found = warnings(
            repo.path(),
            &request(&["aux.txt", "co:lon.txt"]),
            &all_warnings(),
        );

        assert_eq!(found, vec![]);
    }

    #[test]
    fn a_name_that_looks_like_a_pattern_is_looked_up_as_a_literal() {
        let repo = crlf_repo("false");
        repo.commit("a*.txt", b"one\n", "star");
        write(&repo, "a*.txt", b"two\n");

        assert_eq!(
            warnings(repo.path(), &request(&["a*.txt"]), &all_warnings()),
            vec![]
        );

        // `a?.txt` is matched by the pattern `a*.txt`, but it is not in HEAD.
        write(&repo, "a?.txt", b"new\n");
        assert_eq!(
            warnings(repo.path(), &request(&["a?.txt"]), &all_warnings()),
            vec![bad("a?.txt", BadNameProblem::InvalidChar)]
        );
    }

    #[test]
    fn without_commits_every_included_file_is_new() {
        let repo = TempRepo::new();
        write(&repo, "aux.txt", b"x\n");

        let found = warnings(repo.path(), &request(&["aux.txt"]), &all_warnings());

        assert_eq!(found, vec![bad("aux.txt", BadNameProblem::Reserved)]);
    }

    #[test]
    fn the_file_name_check_can_be_turned_off() {
        let repo = crlf_repo("false");
        write(&repo, "aux.txt", b"x\n");
        let settings = Settings {
            warn_bad_file_names: false,
            ..Default::default()
        };

        assert_eq!(
            warnings(repo.path(), &request(&["aux.txt"]), &settings),
            vec![]
        );
    }

    #[test]
    fn in_stage_mode_only_the_added_files_are_checked() {
        let repo = crlf_repo("false");
        repo.commit("co:lon.txt", b"one\n", "colon");
        stage(&repo, "co:lon.txt", b"two\n");
        stage(&repo, "aux.txt", b"x\n");
        write(&repo, "unstaged:.txt", b"x\n");

        let found = staged_warnings(repo.path(), &all_warnings());

        assert_eq!(found, vec![bad("aux.txt", BadNameProblem::Reserved)]);
    }

    #[test]
    fn warnings_come_in_intellijs_order() {
        let (repo, settings) = small_limit();
        repo.commit("a.txt", b"x\n", "second");
        repo.git(&["checkout", "-q", "--detach"]);
        // One file that triggers the CRLF, the size and the name checks.
        let mut content = b"one\r\n".to_vec();
        content.resize(MB + 1, b'x');
        write(&repo, "aux.txt", &content);

        let found = warnings(repo.path(), &request(&["aux.txt"]), &settings);

        assert_eq!(
            found,
            vec![
                CommitWarning::Crlf(strings(&["aux.txt"])),
                large("aux.txt", MB + 1),
                CommitWarning::DetachedHead,
                bad("aux.txt", BadNameProblem::Reserved),
            ]
        );
    }

    #[test]
    fn staged_warnings_come_in_the_same_order() {
        let (repo, settings) = small_limit();
        repo.git(&["checkout", "-q", "--detach"]);
        let mut content = b"one\r\n".to_vec();
        content.resize(MB + 1, b'x');
        stage(&repo, "aux.txt", &content);

        let found = staged_warnings(repo.path(), &settings);

        assert_eq!(
            found,
            vec![
                CommitWarning::Crlf(strings(&["aux.txt"])),
                large("aux.txt", MB + 1),
                CommitWarning::DetachedHead,
                bad("aux.txt", BadNameProblem::Reserved),
            ]
        );
    }

    // A merge of `other` into `main` with the conflict already resolved and marked.
    fn resolved_merge() -> RepoHarness {
        let harness = RepoHarness::new();
        harness.repo.conflict("a.txt");
        harness.write("a.txt", "resolved\n");
        harness.git(&["add", "a.txt"]);
        harness
    }

    #[test]
    fn merge_commit_uses_merge_msg() {
        let harness = resolved_merge();
        fs::write(
            harness.path().join(".git/MERGE_MSG"),
            "Merge branch 'other' with my text\n",
        )
        .unwrap();

        harness.run(merge_commit).unwrap();

        assert_eq!(
            harness.git(&["log", "-1", "--format=%B"]).trim(),
            "Merge branch 'other' with my text"
        );
        let parents = harness.git(&["rev-list", "--parents", "-n", "1", "HEAD"]);
        assert_eq!(parents.split_whitespace().count(), 3, "{parents}");
        assert!(!harness.path().join(".git/MERGE_HEAD").exists());
    }

    #[test]
    fn merge_commit_without_merge_msg_uses_intellijs_text() {
        let harness = resolved_merge();
        fs::remove_file(harness.path().join(".git/MERGE_MSG")).unwrap();

        harness.run(merge_commit).unwrap();

        let expected = format!(
            "Merge branch 'main' of {} with conflicts.",
            harness.path().display()
        );
        assert_eq!(harness.git(&["log", "-1", "--format=%s"]).trim(), expected);
    }

    // `main` and `side` clash in `a.txt`; `git merge --squash side` leaves the conflict without `MERGE_HEAD`.
    fn squashing() -> RepoHarness {
        let harness = RepoHarness::new();
        harness.diverged();
        harness.git_fails(&["merge", "--squash", "side"]);
        harness
    }

    #[test]
    fn merge_commit_of_a_resolved_squash_commits_with_squash_msg() {
        let harness = squashing();
        assert!(!harness.path().join(".git/MERGE_HEAD").exists());
        // Git leaves a `MERGE_MSG` that only carries the comments about the conflicts: the text is the squash one.
        harness.write("a.txt", "resolved\n");
        harness.git(&["add", "a.txt"]);
        let before = harness.head();

        harness.run(merge_commit).unwrap();

        let message = harness.git(&["log", "-1", "--format=%B"]);
        assert!(
            message.starts_with("Squashed commit of the following"),
            "{message}"
        );
        // A normal commit: the squash leaves no merge.
        let parents = harness.git(&["rev-list", "--parents", "-n", "1", "HEAD"]);
        assert_eq!(parents.split_whitespace().count(), 2, "{parents}");
        assert_eq!(harness.git(&["rev-parse", "HEAD~1"]).trim(), before);
        assert!(harness.git(&["status", "--porcelain"]).is_empty());
    }

    #[test]
    fn merge_commit_with_unresolved_paths_refuses_in_merge_and_in_squash() {
        let merging = RepoHarness::new();
        merging.diverged();
        merging.git_fails(&["merge", "side"]);
        let squashing = squashing();

        for harness in [&merging, &squashing] {
            let before = harness.head();

            let failure = harness.run(merge_commit);

            assert!(failure.is_err());
            assert_eq!(harness.head(), before);
            assert!(!harness.ran("commit -F") && !harness.ran("commit -m"));
        }
    }

    #[test]
    fn merge_commit_without_a_merge_in_progress_commits_nothing() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"1\n", "one");
        harness.write("a.txt", "2\n");
        harness.git(&["add", "a.txt"]);

        let failure = harness.run(merge_commit).unwrap_err();

        assert_eq!(failure, GitError::Io("No merge in progress".to_string()));
        assert_eq!(harness.git(&["log", "-1", "--format=%s"]).trim(), "one");
    }

    #[test]
    fn commit_hooks_are_an_executable_pre_commit_or_commit_msg() {
        let repo = TempRepo::new();
        assert!(!has_commit_hooks(repo.path()));

        hook(&repo, "pre-push", "exit 0");
        assert!(!has_commit_hooks(repo.path()));

        for name in ["pre-commit", "commit-msg"] {
            hook(&repo, name, "exit 0");
            assert!(has_commit_hooks(repo.path()), "{name}");
            let script = repo.path().join(".git/hooks").join(name);
            fs::set_permissions(&script, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(!has_commit_hooks(repo.path()), "{name}");
        }
    }

    #[test]
    fn commit_hooks_are_looked_for_where_git_says() {
        let repo = TempRepo::new();
        let hooks = TempDir::new().unwrap();
        let script = hooks.path().join("commit-msg");
        fs::write(&script, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

        repo.git(&["config", "core.hooksPath", hooks.path().to_str().unwrap()]);

        assert!(has_commit_hooks(repo.path()));
    }

    #[test]
    fn merge_message_reads_merge_msg() {
        let harness = RepoHarness::new();
        harness.repo.conflict("a.txt");

        let message = merge_message(harness.path()).unwrap().unwrap();

        assert!(message.starts_with("Merge branch 'other'"), "{message}");
    }

    #[test]
    fn merge_message_reads_squash_msg_if_there_is_no_merge() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        fs::write(
            repo.path().join(".git/SQUASH_MSG"),
            "Squashed commit of 'x'\n",
        )
        .unwrap();

        assert_eq!(
            merge_message(repo.path()).unwrap().as_deref(),
            Some("Squashed commit of 'x'\n")
        );
    }

    #[test]
    fn merge_message_prefers_merge_msg_and_without_files_there_is_no_message() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        assert_eq!(merge_message(repo.path()).unwrap(), None);

        fs::write(repo.path().join(".git/SQUASH_MSG"), "squash\n").unwrap();
        fs::write(repo.path().join(".git/MERGE_MSG"), "merge\n").unwrap();

        assert_eq!(
            merge_message(repo.path()).unwrap().as_deref(),
            Some("merge\n")
        );
    }
}
