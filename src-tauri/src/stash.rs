// The stash of the Git menu, the staging area and the Stash tab: `GitStashUtils` and `GitStashOperations`.
// An entry is named by its hash: what someone else saves in the meantime moves the positions, so the
// `stash@{n}` that git needs is looked up at the moment of the call, and an entry that is gone is refused.
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::branches::{self, Repo};
use crate::diff::{self, ChangedFile, FileSides};
use crate::git::{self, GitError};
use crate::operation;
use crate::preserve;
use crate::snapshots;
use crate::storage;
use crate::task::TaskContext;

// `loadStashStack`: hash, parents, author time, `stash@{n}` and the subject, which goes last because
// it may contain `:`.
const LIST_FORMAT: &str = "--pretty=format:%H:%P:%at:%gd:%s";
const LIST_FIELDS: usize = 5;
// `VcsLogUtil.HASH_REGEX`: what `StashInfo.subject` takes off the start of the message.
const HASH_MIN: usize = 7;
const HASH_MAX: usize = 40;
const NO_BRANCH: &str = "(no branch)";
// `GitSimpleEventDetector.Event.INDEX_CONFLICT_ON_UNSTASH`.
const CONFLICTS_IN_INDEX: &str = "conflicts in index";
const UNSTASH_REASON: &str = "Unstash";
const ENTRY_GONE: &str = "The stash entry is no longer there: nothing was touched";
const NOTHING_TO_STASH: &str = "There are no files to stash";
// What the Conflicts dialog and the merge window need to know about the unstash that left the
// conflicts. It lives in the repository's data folder: the conflicts outlive the notification.
const CONFLICTS_FILE: &str = "unstash-conflicts.json";
// What Drop and Clear removed, in the repository's data folder (`DroppedStash`).
const DROPPED_FILE: &str = "dropped-stashes.json";
const DROPPED_CORRUPT: &str = "dropped-stashes.json.corrupt";
// `UnstashMergeDialogCustomizer`, with `MergeDialogCustomizer`'s center title.
const LEFT_TITLE: &str = "Local changes";
const CENTER_TITLE: &str = "Result";
const RIGHT_TITLE: &str = "Changes from stash";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StashEntry {
    pub sha: String,
    // As git gives them: the commit it was made on, then the index and, if it has them, the
    // untracked files.
    pub parents: Vec<String>,
    // Author time, in milliseconds.
    pub time: u64,
    // The `n` of `stash@{n}` when it was listed.
    pub index: u32,
    // `StashInfo.subject`: the message without the hash it starts with.
    pub subject: String,
    // `StashInfo.branchName`.
    pub branch: Option<String>,
    // `StashInfo.message` and `StashInfo.branch` as git wrote them (`WIP on main`): the confirmations
    // and the rows of the Unstash dialog show them.
    pub message: String,
    pub prefix: Option<String>,
}

impl StashEntry {
    pub fn label(&self) -> String {
        format!("stash@{{{}}}", self.index)
    }
}

// The changes of a stash: its commit against the first parent, then those of the index and of the
// untracked files, each under its own title (`GitStashProvider.toChangeObjects`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StashChanges {
    pub files: Vec<ChangedFile>,
    pub groups: Vec<StashGroup>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StashGroup {
    // `Index on main`, `Untracked files on main`.
    pub title: String,
    // The parent's commit: `file_sides` takes it to show one of these files.
    pub sha: String,
    pub files: Vec<ChangedFile>,
}

// How an unstash ended, in the order `GitStashOperations.unstash` looks at it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "reason", rename_all = "camelCase")]
pub enum Unstashed {
    Applied,
    // What git said. Only resolved by hand: nothing was applied.
    IndexConflict(String),
    // There are unresolved paths, and the entry is still in the stash.
    Conflicts,
    Untracked { files: Vec<String> },
    LocalChanges { files: Vec<String> },
    Failed(String),
}

// The texts of the Conflicts dialog and of the merge window for the conflicts of an unstash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictTexts {
    pub description: String,
    pub titles: [String; 3],
}

#[derive(Debug, Serialize, Deserialize)]
struct ConflictsOrigin {
    stash: String,
    message: String,
    head: String,
    // The paths the unstash left unresolved.
    #[serde(default)]
    paths: BTreeSet<String>,
}

pub fn list(ctx: &TaskContext) -> Result<Vec<StashEntry>, GitError> {
    let output = ctx.git(&["stash", "list", LIST_FORMAT])?;
    Ok(parse_list(&output.stdout))
}

// `createStashHandler`: `stash save`, which takes the message as its last argument. One that starts
// with `-` would be an option there, so it goes with `stash push -m`, which saves the same.
pub fn save(ctx: &TaskContext, message: &str, keep_index: bool) -> Result<(), GitError> {
    let message = message.trim();
    let dashed = message.starts_with('-');
    let mut args = vec!["stash", if dashed { "push" } else { "save" }];
    if keep_index {
        args.push("--keep-index");
    }
    if dashed {
        args.push("-m");
    }
    if !message.is_empty() {
        args.push(message);
    }
    ctx.git_to_the_end(&args)?;
    Ok(())
}

// `Stash Files` (`createStashPushHandler` with `-u`): the paths go on the input, each one a literal
// (`--literal-pathspecs`): a file named `*.txt` is that file, not every text file.
pub fn push_paths(ctx: &TaskContext, paths: &[String]) -> Result<(), GitError> {
    if paths.is_empty() {
        return Err(branches::invalid(NOTHING_TO_STASH.to_string()));
    }
    for path in paths {
        snapshots::check_path(ctx.repo(), path).map_err(git::io_error)?;
    }
    let mut input = Vec::new();
    for path in paths {
        input.extend_from_slice(path.as_bytes());
        input.push(0);
    }
    let args = git::exact(&[
        "--literal-pathspecs",
        "stash",
        "push",
        "-u",
        "--pathspec-from-file=-",
        "--pathspec-file-nul",
    ]);
    ctx.git_with_input(&args, &input)?;
    Ok(())
}

// `createUnstashHandler` with both options honoured (decision 11), and the results of
// `GitStashOperations.unstash`. A recoverable copy of what it may write is taken first.
pub fn unstash(
    repo: &Repo,
    sha: &str,
    pop: bool,
    reinstate_index: bool,
    branch: Option<&str>,
    expected_head: &str,
) -> Result<Unstashed, GitError> {
    let ctx = repo.ctx;
    operation::check_head(repo.path, expected_head)?;
    let entry = find(ctx, sha)?;
    let branch = branch.map(str::trim).filter(|name| !name.is_empty());
    if let Some(name) = branch {
        branches::check_branch(ctx, name)?;
    }
    let label = entry.label();
    let mut args = vec!["stash"];
    match branch {
        Some(name) => args.extend(["branch", name]),
        None => {
            args.push(if pop { "pop" } else { "apply" });
            if reinstate_index {
                args.push("--index");
            }
        }
    }
    args.push(&label);
    // What was unresolved before is not this unstash's: git refuses, and its text is the failure.
    let unmerged_before = preserve::has_unmerged(ctx)?;
    branches::copy_before(repo, UNSTASH_REASON, |path| touched(path, &entry))?;
    forget_conflicts(repo.data);
    let error = match ctx.git_to_the_end(&args) {
        Ok(_) => return Ok(Unstashed::Applied),
        Err(error) => error,
    };
    let text = preserve::failed_text(error)?;
    if text.to_lowercase().contains(CONFLICTS_IN_INDEX) {
        return Ok(Unstashed::IndexConflict(text));
    }
    if !unmerged_before && preserve::has_unmerged(ctx)? {
        remember_conflicts(repo, &entry);
        return Ok(Unstashed::Conflicts);
    }
    let untracked = preserve::listed_files(&text, preserve::UNTRACKED_FILES);
    if !untracked.is_empty() {
        return Ok(Unstashed::Untracked { files: untracked });
    }
    let local = preserve::listed_files(&text, branches::LOCAL_CHANGES);
    if !local.is_empty() {
        return Ok(Unstashed::LocalChanges { files: local });
    }
    Ok(Unstashed::Failed(text))
}

// For the title of the drop: `None` if it is not a stash entry (the drop itself refuses it).
pub fn label_of(repo: &Path, sha: &str) -> Option<String> {
    diff::check_hash(sha).ok()?;
    let output = git::run(repo, &["stash", "list", "--format=%H"]).ok()?;
    let text = String::from_utf8_lossy(&output);
    let position = text.lines().position(|line| line == sha)?;
    Some(format!("stash@{{{position}}}"))
}

// rama's own trail of what Drop and Clear removed (IntelliJ only asks first): git keeps the
// objects until it prunes them, and `git stash store -m <message> <sha>` brings an entry back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DroppedStash {
    pub sha: String,
    // The reflog message as git wrote it (`On main: work`), which `stash store -m` takes.
    pub message: String,
    // When it was dropped, in milliseconds.
    pub time: u64,
}

// Each dropped entry is appended to the record before git drops it: without the record nothing
// is dropped.
fn record_dropped(data: &Path, entries: &[StashEntry]) -> Result<(), GitError> {
    let mut dropped: Vec<DroppedStash> =
        storage::load_json(data, DROPPED_FILE, DROPPED_CORRUPT).map_err(git::io_error)?;
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64);
    dropped.extend(entries.iter().map(|entry| DroppedStash {
        sha: entry.sha.clone(),
        message: match &entry.prefix {
            Some(prefix) => format!("{prefix}: {}", entry.message),
            None => entry.message.clone(),
        },
        time,
    }));
    let bytes =
        serde_json::to_vec_pretty(&dropped).map_err(|error| GitError::Io(error.to_string()))?;
    fs::create_dir_all(data).map_err(git::io_error)?;
    storage::write_atomic(&data.join(DROPPED_FILE), &bytes).map_err(git::io_error)
}

// What was dropped, oldest first. A missing or damaged record is empty.
pub fn dropped_stashes(data: &Path) -> Vec<DroppedStash> {
    fs::read(data.join(DROPPED_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn drop(repo: &Repo, sha: &str) -> Result<(), GitError> {
    let entry = find(repo.ctx, sha)?;
    record_dropped(repo.data, std::slice::from_ref(&entry))?;
    repo.ctx
        .git_to_the_end(&["stash", "drop", &entry.label()])?;
    Ok(())
}

pub fn clear(repo: &Repo) -> Result<(), GitError> {
    record_dropped(repo.data, &list(repo.ctx)?)?;
    repo.ctx.git_to_the_end(&["stash", "clear"])?;
    Ok(())
}

// `GitStashOperations.loadStashChanges`: the stash against its first parent, and each other parent
// against its own first parent (the untracked files have none: everything is added).
pub fn changes(ctx: &TaskContext, sha: &str) -> Result<StashChanges, GitError> {
    let entry = find(ctx, sha)?;
    let repo = ctx.repo();
    let files = diff::commit_files(repo, &entry.sha)?;
    let mut groups = Vec::new();
    for parent in entry.parents.iter().skip(1) {
        ctx.check_cancelled()?;
        let subject = ctx.git(&["show", "-s", "--format=%s", parent])?.stdout;
        groups.push(StashGroup {
            title: capitalize(subject.trim().split(':').next().unwrap_or_default()),
            sha: parent.clone(),
            files: diff::commit_files(repo, parent)?,
        });
    }
    Ok(StashChanges { files, groups })
}

// `sha` is a stash or one of the parents `changes` gave: nothing else of the repository is read here.
pub fn file_sides(
    ctx: &TaskContext,
    sha: &str,
    path: &str,
    old_path: Option<&str>,
) -> Result<FileSides, GitError> {
    diff::check_hash(sha)?;
    let repo = ctx.repo();
    for checked in [Some(path), old_path].into_iter().flatten() {
        snapshots::check_path(repo, checked).map_err(git::io_error)?;
    }
    let known = list(ctx)?
        .into_iter()
        .any(|entry| entry.sha == sha || entry.parents.iter().skip(1).any(|parent| parent == sha));
    if !known {
        return Err(branches::invalid(ENTRY_GONE.to_string()));
    }
    diff::file_sides(repo, sha, path, old_path)
}

// The texts of `UnstashMergeDialogCustomizer` while the conflicts are those of the last unstash: with
// HEAD where it left it, and only its paths unresolved. Without unresolved paths the record is
// forgotten: those conflicts are over.
pub fn conflict_texts(repo: &Path, data: &Path) -> Option<ConflictTexts> {
    let bytes = fs::read(data.join(CONFLICTS_FILE)).ok()?;
    let origin: ConflictsOrigin = serde_json::from_slice(&bytes).ok()?;
    let unmerged = unmerged_paths(repo).ok()?;
    if unmerged.is_empty() {
        forget_conflicts(data);
        return None;
    }
    if operation::head_of(repo).ok()? != origin.head || !unmerged.is_subset(&origin.paths) {
        return None;
    }
    Some(ConflictTexts {
        description: format!(
            "Conflicts during unstashing {}\"{}\"",
            origin.stash, origin.message
        ),
        titles: [LEFT_TITLE, CENTER_TITLE, RIGHT_TITLE].map(str::to_string),
    })
}

// After a resolution: once nothing is unresolved, the unstash's conflicts are over.
pub fn forget_resolved(repo: &Path, data: &Path) {
    if unmerged_paths(repo).is_ok_and(|paths| paths.is_empty()) {
        forget_conflicts(data);
    }
}

// `ls-files --unmerged`: one line per stage, `<mode> <hash> <stage>\t<path>`.
fn unmerged_paths(repo: &Path) -> Result<BTreeSet<String>, GitError> {
    let output = git::run(repo, &["ls-files", "--unmerged", "-z"])?;
    Ok(output
        .split(|byte| *byte == 0)
        .filter_map(|entry| {
            let tab = entry.iter().position(|byte| *byte == b'\t')?;
            Some(String::from_utf8_lossy(&entry[tab + 1..]).into_owned())
        })
        .collect())
}

// It is an aid for the texts: if it cannot be written, the unstash is not stopped because of that.
fn remember_conflicts(repo: &Repo, entry: &StashEntry) {
    let (Ok(head), Ok(paths)) = (operation::head_of(repo.path), unmerged_paths(repo.path)) else {
        return;
    };
    let origin = ConflictsOrigin {
        stash: entry.label(),
        message: entry.message.clone(),
        head,
        paths,
    };
    if let Ok(json) = serde_json::to_vec_pretty(&origin) {
        let _ = fs::create_dir_all(repo.data);
        let _ = storage::write_atomic(&repo.data.join(CONFLICTS_FILE), &json);
    }
}

fn forget_conflicts(data: &Path) {
    let _ = fs::remove_file(data.join(CONFLICTS_FILE));
}

fn find(ctx: &TaskContext, sha: &str) -> Result<StashEntry, GitError> {
    diff::check_hash(sha)?;
    list(ctx)?
        .into_iter()
        .find(|entry| entry.sha == sha)
        .ok_or_else(|| branches::invalid(ENTRY_GONE.to_string()))
}

// What the unstash may write: the local changes, and every path the stash and its parents touch.
fn touched(repo: &Path, entry: &StashEntry) -> Result<Vec<String>, GitError> {
    let mut paths: BTreeSet<String> = snapshots::at_risk(repo, "HEAD")?.into_iter().collect();
    let commits = std::iter::once(&entry.sha).chain(entry.parents.iter().skip(1));
    for commit in commits {
        for file in diff::commit_files(repo, commit)? {
            paths.extend(file.old_path);
            paths.insert(file.path);
        }
    }
    Ok(paths.into_iter().collect())
}

// `StringUtil.capitalize`.
fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

// `StashInfo.subject`.
fn subject_of(message: &str) -> String {
    let hex = message.bytes().take_while(u8::is_ascii_hexdigit).count();
    let start = if hex >= HASH_MIN {
        hex.min(HASH_MAX)
    } else {
        0
    };
    message[start..].trim().to_string()
}

// `StashInfo.branchName`.
fn branch_of(prefix: &str) -> Option<String> {
    if prefix.ends_with(NO_BRANCH) {
        return None;
    }
    prefix.split(' ').next_back().map(str::to_string)
}

// `loadStashStack`: a line that cannot be read is skipped.
fn parse_list(text: &str) -> Vec<StashEntry> {
    let mut entries = Vec::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.splitn(LIST_FIELDS + 1, ':').collect();
        if parts.len() < LIST_FIELDS {
            continue;
        }
        let index = parts[3]
            .strip_prefix("stash@{")
            .and_then(|rest| rest.strip_suffix('}'))
            .and_then(|number| number.parse().ok());
        let (Some(index), Ok(seconds)) = (index, parts[2].parse::<u64>()) else {
            continue;
        };
        let prefix = (parts.len() > LIST_FIELDS).then(|| parts[4].trim().to_string());
        let message = parts[parts.len() - 1].trim().to_string();
        entries.push(StashEntry {
            sha: parts[0].to_string(),
            parents: parts[1].split_whitespace().map(str::to_string).collect(),
            time: seconds * 1000,
            index,
            subject: subject_of(&message),
            branch: prefix.as_deref().and_then(branch_of),
            message,
            prefix,
        });
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::RepoHarness;

    fn listed(harness: &RepoHarness) -> Vec<StashEntry> {
        harness.run(|repo| list(repo.ctx)).unwrap()
    }

    fn top(harness: &RepoHarness) -> String {
        harness.git(&["rev-parse", "refs/stash"]).trim().to_string()
    }

    fn unstash_with(
        harness: &RepoHarness,
        sha: &str,
        pop: bool,
        reinstate_index: bool,
        branch: Option<&str>,
    ) -> Result<Unstashed, GitError> {
        let (sha, head, branch) = (sha.to_string(), harness.head(), branch.map(str::to_string));
        harness.run(move |repo| unstash(repo, &sha, pop, reinstate_index, branch.as_deref(), &head))
    }

    #[test]
    fn save_then_list_then_pop_round_trip() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"one\n", "first");
        harness.write("a.txt", "two\n");

        harness
            .run(|repo| save(repo.ctx, "  my work  ", false))
            .unwrap();
        let entries = listed(&harness);

        assert_eq!(harness.read("a.txt"), "one\n");
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.sha, top(&harness));
        assert_eq!(entry.index, 0);
        assert_eq!(entry.label(), "stash@{0}");
        assert_eq!(entry.subject, "my work");
        assert_eq!(entry.message, "my work");
        assert_eq!(entry.prefix.as_deref(), Some("On main"));
        assert_eq!(entry.branch.as_deref(), Some("main"));
        assert_eq!(entry.parents.len(), 2);
        assert!(entry.time > 1_000_000_000_000);

        let outcome = unstash_with(&harness, &entry.sha, true, false, None).unwrap();

        assert_eq!(outcome, Unstashed::Applied);
        assert_eq!(harness.read("a.txt"), "two\n");
        assert!(listed(&harness).is_empty());
    }

    #[test]
    fn a_stash_without_message_shows_the_commit_subject() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"one\n", "first: commit");
        harness.write("a.txt", "two\n");

        harness.run(|repo| save(repo.ctx, "", true)).unwrap();
        let entry = listed(&harness).remove(0);

        assert_eq!(entry.prefix.as_deref(), Some("WIP on main"));
        assert!(
            entry.message.ends_with(" first: commit"),
            "{}",
            entry.message
        );
        assert_eq!(entry.subject, "first: commit");
        assert!(harness.ran("stash save --keep-index"));
    }

    #[test]
    fn a_message_starting_with_dash_goes_through_push_m() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"one\n", "first");
        harness.write("a.txt", "two\n");

        harness
            .run(|repo| save(repo.ctx, "-p oops", false))
            .unwrap();

        assert!(harness.ran("stash push -m -p oops"));
        assert_eq!(listed(&harness)[0].subject, "-p oops");
        assert_eq!(harness.read("a.txt"), "one\n");
    }

    // Decision 11: both checkboxes are honoured, `Reinstate index` passes `--index` and `Pop stash`
    // decides between pop and apply.
    #[test]
    fn unstash_with_reinstate_index_passes_index() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"one\n", "first");
        harness.write("a.txt", "staged\n");
        harness.git(&["add", "a.txt"]);
        harness.write("a.txt", "disk\n");
        harness.git(&["stash", "save", "both"]);
        let sha = top(&harness);

        let outcome = unstash_with(&harness, &sha, false, true, None).unwrap();

        assert_eq!(outcome, Unstashed::Applied);
        assert!(harness.ran("stash apply --index stash@{0}"));
        assert_eq!(harness.git(&["show", ":a.txt"]), "staged\n");
        assert_eq!(harness.read("a.txt"), "disk\n");
        assert_eq!(listed(&harness).len(), 1, "apply keeps the entry");

        harness.git(&["reset", "-q", "--hard"]);
        let outcome = unstash_with(&harness, &sha, true, false, None).unwrap();

        assert_eq!(outcome, Unstashed::Applied);
        assert!(harness.ran("stash pop stash@{0}"));
        assert_eq!(harness.git(&["show", ":a.txt"]), "one\n");
        assert!(listed(&harness).is_empty(), "pop removes the entry");
    }

    #[test]
    fn unstash_as_new_branch() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"one\n", "first");
        harness.write("a.txt", "two\n");
        harness.git(&["stash", "save", "work"]);
        harness.repo.commit("b.txt", b"later\n", "second");
        let sha = top(&harness);

        let outcome = unstash_with(&harness, &sha, false, false, Some("rescued")).unwrap();

        assert_eq!(outcome, Unstashed::Applied);
        assert_eq!(harness.git(&["branch", "--show-current"]).trim(), "rescued");
        assert_eq!(harness.read("a.txt"), "two\n");
        assert!(!harness.path().join("b.txt").exists());
        assert!(listed(&harness).is_empty());
    }

    #[test]
    fn a_branch_name_that_is_an_option_is_refused() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"one\n", "first");
        harness.write("a.txt", "two\n");
        harness.git(&["stash", "save", "work"]);
        let sha = top(&harness);

        let outcome = unstash_with(&harness, &sha, false, false, Some("-f"));

        assert!(outcome.is_err());
        assert!(!harness.ran("stash branch"));
        assert_eq!(listed(&harness).len(), 1);
    }

    #[test]
    fn drop_by_sha_when_positions_moved() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"one\n", "first");
        harness.write("a.txt", "two\n");
        harness.git(&["stash", "save", "older"]);
        let older = top(&harness);
        harness.write("a.txt", "three\n");
        harness.git(&["stash", "save", "newer"]);

        harness.run(move |repo| drop(repo, &older)).unwrap();
        let left = listed(&harness);

        assert!(harness.ran("stash drop stash@{1}"));
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].subject, "newer");
    }

    #[test]
    fn an_entry_that_is_gone_is_refused() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"one\n", "first");
        harness.write("a.txt", "two\n");
        harness.git(&["stash", "save", "work"]);
        let sha = top(&harness);
        harness.git(&["stash", "drop"]);

        let dropped = harness.run({
            let sha = sha.clone();
            move |repo| drop(repo, &sha)
        });
        let unstashed = unstash_with(&harness, &sha, true, false, None);

        assert!(dropped.is_err());
        assert!(unstashed.is_err());
        assert_eq!(harness.snapshots(), 0);
    }

    // Finding 5: before a drop or a clear, each entry's hash, message (as git wrote it) and the
    // time go to `dropped-stashes.json`; `git stash store -m <message> <sha>` brings one back
    // until git prunes its objects.
    #[test]
    fn a_dropped_or_cleared_stash_is_recorded_first_and_can_be_stored_again() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"one\n", "first");
        harness.write("a.txt", "two\n");
        harness.git(&["stash", "save", "older"]);
        let older = top(&harness);
        harness.write("a.txt", "three\n");
        harness.git(&["stash", "save", "newer"]);
        let newer = top(&harness);

        harness
            .run({
                let older = older.clone();
                move |repo| drop(repo, &older)
            })
            .unwrap();
        let after_drop = dropped_stashes(harness.data.path());
        harness.run(clear).unwrap();
        let after_clear = dropped_stashes(harness.data.path());

        assert_eq!(after_drop.len(), 1);
        assert_eq!(after_drop[0].sha, older);
        assert_eq!(after_drop[0].message, "On main: older");
        assert!(after_drop[0].time > 1_000_000_000_000);
        let shas: Vec<&str> = after_clear.iter().map(|entry| entry.sha.as_str()).collect();
        assert_eq!(shas, [older.as_str(), newer.as_str()]);
        assert!(listed(&harness).is_empty());

        let back = &after_clear[0];
        harness.git(&["stash", "store", "-m", &back.message, &back.sha]);
        let listed = listed(&harness);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].sha, older);
        assert_eq!(listed[0].subject, "older");
    }

    // Without the record nothing is dropped.
    #[test]
    fn a_drop_whose_record_cannot_be_written_drops_nothing() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"one\n", "first");
        harness.write("a.txt", "two\n");
        harness.git(&["stash", "save", "work"]);
        let sha = top(&harness);
        fs::create_dir_all(harness.data.path().join("dropped-stashes.json")).unwrap();

        let dropped = harness.run(move |repo| drop(repo, &sha));
        let cleared = harness.run(clear);

        assert!(dropped.is_err());
        assert!(cleared.is_err());
        assert_eq!(listed(&harness).len(), 1);
    }

    #[test]
    fn clear_removes_every_entry() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"one\n", "first");
        harness.write("a.txt", "two\n");
        harness.git(&["stash", "save", "a"]);
        harness.write("a.txt", "three\n");
        harness.git(&["stash", "save", "b"]);

        harness.run(clear).unwrap();

        assert!(listed(&harness).is_empty());
    }

    #[test]
    fn index_conflict_says_unstash_failed_due_to_an_index_conflict() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"base\n", "base");
        harness.write("a.txt", "staged\n");
        harness.git(&["add", "a.txt"]);
        harness.write("a.txt", "disk\n");
        harness.git(&["stash", "save", "work"]);
        harness.repo.commit("a.txt", b"committed\n", "two");
        let sha = top(&harness);

        let outcome = unstash_with(&harness, &sha, true, true, None).unwrap();

        let Unstashed::IndexConflict(text) = outcome else {
            panic!("{outcome:?}");
        };
        assert!(text.to_lowercase().contains("conflicts in index"), "{text}");
        assert_eq!(harness.read("a.txt"), "committed\n");
        assert_eq!(listed(&harness).len(), 1);
    }

    #[test]
    fn conflicts_leave_the_entry_and_return_unmerged() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"base\n", "base");
        harness.write("a.txt", "stashed\n");
        harness.git(&["stash", "save", "my work"]);
        harness.repo.commit("a.txt", b"committed\n", "two");
        let sha = top(&harness);

        let outcome = unstash_with(&harness, &sha, true, false, None).unwrap();

        assert_eq!(outcome, Unstashed::Conflicts);
        assert!(!harness.git(&["ls-files", "--unmerged"]).is_empty());
        assert_eq!(
            listed(&harness).len(),
            1,
            "a pop with conflicts keeps the entry"
        );
        // `UnstashMergeDialogCustomizer`: the Conflicts dialog and the merge window name the stash.
        let texts = conflict_texts(harness.path(), harness.data.path()).unwrap();
        assert_eq!(
            texts.description,
            "Conflicts during unstashing stash@{0}\"my work\""
        );
        assert_eq!(
            texts.titles,
            ["Local changes", "Result", "Changes from stash"].map(str::to_string)
        );
        let (session, sides) = harness.run(|repo| {
            let session = crate::conflicts::session(repo).unwrap();
            (session, crate::conflicts::sides(repo, "a.txt").unwrap())
        });
        assert_eq!(session.description, texts.description);
        assert_eq!(sides.titles, texts.titles);
    }

    #[test]
    fn the_unstash_texts_do_not_outlive_their_head() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"base\n", "base");
        harness.write("a.txt", "stashed\n");
        harness.git(&["stash", "save", "my work"]);
        harness.repo.commit("a.txt", b"committed\n", "two");
        let sha = top(&harness);
        unstash_with(&harness, &sha, false, false, None).unwrap();

        harness.git(&["reset", "-q", "--hard"]);
        harness.repo.commit("b.txt", b"b\n", "three");

        assert_eq!(conflict_texts(harness.path(), harness.data.path()), None);
    }

    // Unresolved paths at the same HEAD, without an operation, as Cherry-Pick Selected Changes leaves
    // them (`git apply --3way`).
    fn apply_conflict(harness: &RepoHarness, file: &str) {
        harness.write(file, "theirs\n");
        let patch = harness.git(&["diff", "--", file]);
        harness.git(&["checkout", "-q", "--", file]);
        harness.write(file, "ours\n");
        harness.git(&["commit", "-qam", "ours"]);
        let patch_file = harness.data.path().join("x.patch");
        fs::write(&patch_file, patch).unwrap();
        let output = crate::test_support::git_command()
            .current_dir(harness.path())
            .args(["apply", "--3way"])
            .arg(&patch_file)
            .output()
            .unwrap();
        assert!(!output.status.success());
    }

    fn unstash_conflict(harness: &RepoHarness) {
        harness.repo.commit("a.txt", b"base\n", "base");
        harness.repo.commit("b.txt", b"base\n", "base b");
        harness.write("a.txt", "stashed\n");
        harness.git(&["stash", "save", "my work"]);
        harness.repo.commit("a.txt", b"committed\n", "two");
        let sha = top(harness);
        let outcome = unstash_with(harness, &sha, false, false, None).unwrap();
        assert_eq!(outcome, Unstashed::Conflicts);
    }

    #[test]
    fn the_unstash_texts_end_with_their_conflicts() {
        let harness = RepoHarness::new();
        unstash_conflict(&harness);
        let marker = harness.data.path().join(CONFLICTS_FILE);
        assert!(marker.exists());

        harness
            .run(|repo| {
                crate::conflicts::resolve(repo, "a.txt", b"resolved\n", &{
                    crate::conflicts::sides(repo, "a.txt").unwrap().hash
                })
            })
            .unwrap();

        assert!(
            !marker.exists(),
            "the marker goes with the last resolved path"
        );
        harness.git(&["reset", "-q"]);
        harness.git(&["checkout", "-q", "--", "."]);
        apply_conflict(&harness, "b.txt");
        let session = harness.run(|repo| crate::conflicts::session(repo).unwrap());
        assert_eq!(session.description, "");
        assert_eq!(conflict_texts(harness.path(), harness.data.path()), None);
    }

    #[test]
    fn other_conflicts_next_to_the_unstash_ones_get_the_generic_texts() {
        let harness = RepoHarness::new();
        unstash_conflict(&harness);
        let head = harness.head();
        // The marker of an unstash at this HEAD whose paths were others.
        let marker = harness.data.path().join(CONFLICTS_FILE);
        let origin = serde_json::json!({
            "stash": "stash@{0}", "message": "my work", "head": head, "paths": ["c.txt"]
        });
        fs::write(&marker, origin.to_string()).unwrap();

        assert_eq!(conflict_texts(harness.path(), harness.data.path()), None);
    }

    #[test]
    fn a_marker_without_unmerged_paths_is_forgotten_when_read() {
        let harness = RepoHarness::new();
        unstash_conflict(&harness);
        harness.git(&["checkout", "-q", "HEAD", "--", "a.txt"]);

        assert_eq!(conflict_texts(harness.path(), harness.data.path()), None);
        assert!(!harness.data.path().join(CONFLICTS_FILE).exists());
    }

    #[test]
    fn stash_branch_takes_a_snapshot_first() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"one\n", "first");
        harness.write("a.txt", "two\n");
        harness.git(&["stash", "save", "work"]);
        let sha = top(&harness);

        let outcome = unstash_with(&harness, &sha, false, false, Some("rescued")).unwrap();

        assert_eq!(outcome, Unstashed::Applied);
        assert_eq!(harness.snapshots(), 1);
        assert_eq!(harness.saved_paths(), ["a.txt"]);
    }

    #[test]
    fn stash_files_takes_names_with_pathspec_magic_literally() {
        let harness = RepoHarness::new();
        let names = ["a.txt", "b.txt", "*.txt", ":(glob)x"];
        for name in names {
            harness.repo.commit(name, b"one\n", name);
        }
        for name in names {
            harness.write(name, "two\n");
        }

        let paths = vec!["*.txt".to_string(), ":(glob)x".to_string()];
        harness
            .run(move |repo| push_paths(repo.ctx, &paths))
            .unwrap();

        assert_eq!(harness.read("*.txt"), "one\n");
        assert_eq!(harness.read(":(glob)x"), "one\n");
        assert_eq!(harness.read("a.txt"), "two\n");
        assert_eq!(harness.read("b.txt"), "two\n");
    }

    #[test]
    fn untracked_would_be_overwritten_lists_the_files() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"base\n", "base");
        harness.write("n.txt", "new\n");
        harness.git(&["add", "n.txt"]);
        harness.git(&["stash", "save", "adds n"]);
        harness.write("n.txt", "someone else's\n");
        let sha = top(&harness);

        let outcome = unstash_with(&harness, &sha, true, false, None).unwrap();

        assert_eq!(
            outcome,
            Unstashed::Untracked {
                files: vec!["n.txt".to_string()]
            }
        );
        assert_eq!(harness.read("n.txt"), "someone else's\n");
        assert_eq!(listed(&harness).len(), 1);
    }

    #[test]
    fn local_changes_would_be_overwritten_lists_the_files() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"base\n", "base");
        harness.write("a.txt", "stashed\n");
        harness.git(&["stash", "save", "work"]);
        harness.write("a.txt", "again\n");
        let sha = top(&harness);

        let outcome = unstash_with(&harness, &sha, false, false, None).unwrap();

        assert_eq!(
            outcome,
            Unstashed::LocalChanges {
                files: vec!["a.txt".to_string()]
            }
        );
        assert_eq!(harness.read("a.txt"), "again\n");
    }

    #[test]
    fn changes_group_index_and_untracked_parents() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"base\n", "base");
        harness.write("a.txt", "staged\n");
        harness.git(&["add", "a.txt"]);
        harness.write("a.txt", "disk\n");
        harness.write("u.txt", "untracked\n");
        harness.git(&["stash", "push", "-u", "-m", "all"]);
        let sha = top(&harness);

        let (changes, sides, index_sides) = harness.run({
            let sha = sha.clone();
            move |repo| {
                let changes = changes(repo.ctx, &sha).unwrap();
                let sides = file_sides(repo.ctx, &sha, "a.txt", None).unwrap();
                let index = changes.groups[0].sha.clone();
                let index_sides = file_sides(repo.ctx, &index, "a.txt", None).unwrap();
                (changes, sides, index_sides)
            }
        });

        let paths = |files: &[ChangedFile]| -> Vec<String> {
            files
                .iter()
                .map(|file| format!("{} {}", file.status, file.path))
                .collect()
        };
        assert_eq!(paths(&changes.files), ["M a.txt"]);
        let titles: Vec<&str> = changes
            .groups
            .iter()
            .map(|group| group.title.as_str())
            .collect();
        assert_eq!(titles, ["Index on main", "Untracked files on main"]);
        assert_eq!(paths(&changes.groups[0].files), ["M a.txt"]);
        assert_eq!(paths(&changes.groups[1].files), ["A u.txt"]);
        let text = |side: &diff::Side| match side {
            diff::Side::Text { content } => content.clone(),
            other => format!("{other:?}"),
        };
        assert_eq!(
            (text(&sides.old), text(&sides.new)),
            ("base\n".into(), "disk\n".into())
        );
        assert_eq!(text(&index_sides.new), "staged\n");
    }

    #[test]
    fn sides_are_only_read_from_a_stash() {
        let harness = RepoHarness::new();
        let head = harness.repo.commit("a.txt", b"base\n", "base");

        let result = harness.run(move |repo| file_sides(repo.ctx, &head, "a.txt", None));

        assert!(result.is_err());
    }

    #[test]
    fn a_snapshot_precedes_unstash() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"one\n", "first");
        harness.repo.commit("b.txt", b"one\n", "second");
        harness.write("a.txt", "stashed\n");
        harness.git(&["stash", "save", "work"]);
        harness.write("b.txt", "local\n");
        let sha = top(&harness);

        let outcome = unstash_with(&harness, &sha, true, false, None).unwrap();

        assert_eq!(outcome, Unstashed::Applied);
        assert_eq!(harness.snapshots(), 1);
        assert_eq!(harness.saved_paths(), ["a.txt", "b.txt"]);
    }

    #[test]
    fn stash_files_takes_only_those_paths_with_untracked_ones() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"one\n", "first");
        harness.repo.commit("b.txt", b"one\n", "second");
        harness.write("a.txt", "two\n");
        harness.write("b.txt", "two\n");
        harness.write("new file.txt", "new\n");

        let paths = vec!["a.txt".to_string(), "new file.txt".to_string()];
        harness
            .run(move |repo| push_paths(repo.ctx, &paths))
            .unwrap();

        assert_eq!(harness.read("a.txt"), "one\n");
        assert_eq!(harness.read("b.txt"), "two\n");
        assert!(!harness.path().join("new file.txt").exists());
        assert_eq!(listed(&harness).len(), 1);
    }

    #[test]
    fn stash_files_refuses_paths_outside_the_repository() {
        let harness = RepoHarness::new();
        harness.repo.commit("a.txt", b"one\n", "first");

        let results = harness.run(|repo| {
            [
                push_paths(repo.ctx, &["../a.txt".to_string()]).is_err(),
                push_paths(repo.ctx, &[]).is_err(),
            ]
        });

        assert_eq!(results, [true, true]);
        assert!(!harness.ran("stash push"));
    }

    #[test]
    fn subjects_and_branches_like_stash_info() {
        let line = |subject: &str| {
            format!(
                "{}:{} {}:1700000000:stash@{{3}}:{subject}",
                "a".repeat(40),
                "b".repeat(40),
                "c".repeat(40)
            )
        };
        let text = [
            line("WIP on main: 1234abcd fix: things"),
            line("On feature/x: plain"),
            line("On (no branch): detached"),
            line("no colon"),
        ]
        .join("\n");

        let entries = parse_list(&text);

        let summary: Vec<(String, Option<String>, Option<String>)> = entries
            .iter()
            .map(|entry| {
                (
                    entry.subject.clone(),
                    entry.branch.clone(),
                    entry.prefix.clone(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (
                    "fix: things".into(),
                    Some("main".into()),
                    Some("WIP on main".into())
                ),
                (
                    "plain".into(),
                    Some("feature/x".into()),
                    Some("On feature/x".into())
                ),
                ("detached".into(), None, Some("On (no branch)".into())),
                ("no colon".into(), None, None),
            ]
        );
        assert_eq!(entries[0].index, 3);
        assert_eq!(entries[0].time, 1_700_000_000_000);
        assert_eq!(entries[0].parents.len(), 2);
    }
}
