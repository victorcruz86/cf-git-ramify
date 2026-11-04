// Save the local changes before an operation and give them back afterwards, like
// `GitPreservingProcess`. They are saved in the stash: a saved change stays in the repository and
// survives a shutdown.
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};

use crate::branches;
use crate::commit;
use crate::git::{self, GitError};
use crate::snapshots;
use crate::storage;
use crate::task::TaskContext;

const STASH_REF: &str = "refs/stash";
const PENDING_FILE: &str = "pending-restores.json";
const PENDING_CORRUPT: &str = "pending-restores.json.corrupt";
const CONFLICTS_IN_INDEX: &str = "conflicts in index";
pub(crate) const UNMERGED_FIRST: &str = "Finish or abort first";
pub(crate) const UNTRACKED_FILES: &str = "untracked working tree files would be overwritten by";

// Reading, changing and writing must not cross with the `pending` of opening the repository.
static PENDING_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Saved {
    pub sha: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "reason", rename_all = "camelCase")]
pub enum RestoreOutcome {
    Restored,
    // What was on disk came back, but the saved stage did not fit. The entry stays: what was only
    // in the stage does not exist anywhere else.
    RestoredWithoutIndex,
    // There are unresolved paths, and the entry is still in the stash.
    Conflicts,
    // Git refused to touch these files and nothing has been applied: the version on disk and what
    // was saved are both intact, each in its own place.
    RestoreBlocked { files: Vec<String> },
    // The entry is no longer in the stash: someone else took it out, and there is nothing to give back.
    Gone,
    Failed(String),
}

// What was saved and has not come back yet. It lives in the repository's data folder because the
// notification does not survive a shutdown, and without it nothing in the app finds the entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pending {
    pub sha: String,
    pub title: String,
    pub message: String,
}

fn read_pending(data: &Path) -> io::Result<Vec<Pending>> {
    let file = data.join(PENDING_FILE);
    let bytes = match fs::read(&file) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    match serde_json::from_slice(&bytes) {
        Ok(list) => Ok(list),
        Err(_) => {
            storage::keep_corrupt(data, &file, PENDING_CORRUPT)?;
            Ok(Vec::new())
        }
    }
}

// It is an aid to find the entry again, which is still in the stash: if it cannot be written, the
// operation is not stopped because of that. A file that could not be read is not overwritten.
fn change_pending(data: &Path, change: impl FnOnce(&mut Vec<Pending>)) {
    let _held = PENDING_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let Ok(mut list) = read_pending(data) else {
        return;
    };
    let before = list.clone();
    change(&mut list);
    if list == before {
        return;
    }
    if let Ok(json) = serde_json::to_vec_pretty(&list) {
        let _ = fs::create_dir_all(data);
        let _ = storage::write_atomic(&data.join(PENDING_FILE), &json);
    }
}

// What was recorded that is still in the stash. What is no longer there, someone took out, and it is forgotten.
pub fn pending(repo: &Path, data: &Path) -> Result<Vec<Pending>, GitError> {
    let output = git::run(repo, &["stash", "list", "--format=%H"])?;
    let text = String::from_utf8_lossy(&output);
    let alive: HashSet<&str> = text.lines().collect();
    change_pending(data, |list| {
        list.retain(|entry| alive.contains(entry.sha.as_str()));
    });
    read_pending(data).map_err(git::io_error)
}

// `restore`, and the record is forgotten if there is nothing left to give back. Without the stage the entry is
// kept, but what was on disk came back: there is nothing to offer on opening.
pub fn restore_tracked(ctx: &TaskContext, data: &Path, saved: &Saved) -> RestoreOutcome {
    let outcome = restore(ctx, saved)
        .unwrap_or_else(|error| RestoreOutcome::Failed(branches::text_of(error)));
    if matches!(
        outcome,
        RestoreOutcome::Restored | RestoreOutcome::RestoredWithoutIndex | RestoreOutcome::Gone
    ) {
        change_pending(data, |list| list.retain(|entry| entry.sha != saved.sha));
    }
    outcome
}

fn stash_top(ctx: &TaskContext) -> Result<Option<String>, GitError> {
    match ctx.git_to_the_end(&["rev-parse", "-q", "--verify", STASH_REF]) {
        Ok(output) => Ok(Some(output.stdout.trim().to_string())),
        Err(GitError::Failed { code: Some(1), .. }) => Ok(None),
        Err(error) => Err(error),
    }
}

// Local time, like `DateFormatUtil.formatDateTime`.
fn now_text() -> String {
    const FORMAT: &[u8] = b"%Y-%m-%d %H:%M:%S\0";
    let mut buffer = [0u8; 64];
    // SAFETY: `tm` belongs to this function and `strftime` writes no more than the size it is given.
    let written = unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm = std::mem::zeroed::<libc::tm>();
        libc::localtime_r(&now, &mut tm);
        libc::strftime(
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            FORMAT.as_ptr().cast(),
            &tm,
        )
    };
    String::from_utf8_lossy(&buffer[..written]).into_owned()
}

// `None` if there was nothing to save. The stash says so, not git's text, which also returns 0.
// Untracked files do not go in: `git stash save` does not touch them.
pub fn save(ctx: &TaskContext, title: &str) -> Result<Option<Saved>, GitError> {
    save_as(ctx, &dated(title))
}

// `GitPreservingProcess`: the title with the time.
fn dated(title: &str) -> String {
    format!("Uncommitted changes before {title} at {}", now_text())
}

fn save_as(ctx: &TaskContext, message: &str) -> Result<Option<Saved>, GitError> {
    ctx.check_cancelled()?;
    let before = stash_top(ctx)?;
    let message = message.to_string();
    ctx.git_to_the_end(&["stash", "save", &message])?;
    let sha = stash_top(ctx)?.filter(|top| Some(top) != before.as_ref());
    Ok(sha.map(|sha| Saved { sha, message }))
}

pub(crate) fn has_unmerged(ctx: &TaskContext) -> Result<bool, GitError> {
    let output = ctx.git_to_the_end(&["ls-files", "--unmerged"])?;
    Ok(!output.stdout.is_empty())
}

fn failed_text(error: GitError) -> Result<String, GitError> {
    match error {
        GitError::Failed { stderr, stdout, .. } => {
            Ok(format!("{}\n{}", stderr.trim(), stdout.trim())
                .trim()
                .to_string())
        }
        error => Err(error),
    }
}

// The files of a git block: a line with `marker` and the paths with a tab.
pub(crate) fn listed_files(stderr: &str, marker: &str) -> Vec<String> {
    let lines = stderr.lines().skip_while(|line| !line.contains(marker));
    let files = lines.skip(1).map_while(|line| line.strip_prefix('\t'));
    files.map(str::to_string).collect()
}

// `stash pop` does not overwrite a file that someone touched or created while it was saved. Nothing
// has been applied: there are no unresolved paths, just two versions, each in its own place.
fn refused(text: String) -> RestoreOutcome {
    let files = if text.contains(UNTRACKED_FILES) {
        listed_files(&text, UNTRACKED_FILES)
    } else {
        listed_files(&text, branches::LOCAL_CHANGES)
    };
    if files.is_empty() {
        RestoreOutcome::Failed(text)
    } else {
        RestoreOutcome::RestoreBlocked { files }
    }
}

// `save`, for whoever gives back afterwards without `--index`: with the staging area off `pop` leaves what was on
// disk, and what was only in the stage would be lost with the entry. Without a copy nothing is saved.
pub fn save_with_index_copy(
    ctx: &TaskContext,
    repo: &Path,
    data: &Path,
    title: &str,
) -> Result<Option<Saved>, GitError> {
    save_with_index_copy_as(ctx, repo, data, title, &dated(title))
}

// With the message given: the rebase saves with `Uncommitted changes before rebase`, without the time,
// like `GitRebaseSpec.newSaver`.
pub fn save_with_index_copy_as(
    ctx: &TaskContext,
    repo: &Path,
    data: &Path,
    title: &str,
    message: &str,
) -> Result<Option<Saved>, GitError> {
    ctx.check_cancelled()?;
    if !ctx.staging_area() {
        let paths = snapshots::index_only(repo)?;
        if !paths.is_empty() {
            snapshots::create_for_index(data, repo, &paths, title).map_err(|error| {
                GitError::Io(format!(
                    "Could not save the backup copy and nothing was touched: {error}"
                ))
            })?;
        }
    }
    let saved = save_as(ctx, message)?;
    if let Some(Saved { sha, message }) = &saved {
        change_pending(data, |list| {
            list.push(Pending {
                sha: sha.clone(),
                title: title.to_string(),
                message: message.clone(),
            });
        });
    }
    Ok(saved)
}

// The entry is looked up by its hash: what someone else saves in the meantime changes the positions. If
// git fails without conflicts, the entry stays and nothing has been applied.
pub fn restore(ctx: &TaskContext, saved: &Saved) -> Result<RestoreOutcome, GitError> {
    let list = ctx.git_to_the_end(&["stash", "list", "--format=%H"])?;
    let Some(position) = list.stdout.lines().position(|sha| sha == saved.sha) else {
        return Ok(RestoreOutcome::Gone);
    };
    // With unresolved paths, `pop` applies nothing (`could not write index`) and the conflicts that
    // would be seen afterwards would not be its own. With a resolved merge not yet committed or a stopped rebase, `pop`
    // does apply, and what was saved would be mixed with its result.
    let repo = ctx.repo();
    if commit::merge_in_progress(repo) || commit::rebase_in_progress(repo) || has_unmerged(ctx)? {
        return Ok(RestoreOutcome::Failed(UNMERGED_FIRST.to_string()));
    }
    let entry = format!("stash@{{{position}}}");
    let with_index = ctx.staging_area();
    let args: &[&str] = if with_index {
        &["stash", "pop", "--index", &entry]
    } else {
        &["stash", "pop", &entry]
    };
    let error = match ctx.git_to_the_end(args) {
        Ok(_) => return Ok(RestoreOutcome::Restored),
        Err(error) => error,
    };
    let text = failed_text(error)?;
    if with_index && text.to_lowercase().contains(CONFLICTS_IN_INDEX) {
        return apply_keeping_entry(ctx, &entry);
    }
    if has_unmerged(ctx)? {
        return Ok(RestoreOutcome::Conflicts);
    }
    Ok(refused(text))
}

// The saved stage does not fit the destination, and `--index` has applied nothing: the changes would not
// be seen. What was on disk comes back, and the entry stays because what was only in the stage does not come back.
fn apply_keeping_entry(ctx: &TaskContext, entry: &str) -> Result<RestoreOutcome, GitError> {
    match ctx.git_to_the_end(&["stash", "apply", entry]) {
        Ok(_) => Ok(RestoreOutcome::RestoredWithoutIndex),
        Err(error) => {
            let text = failed_text(error)?;
            if has_unmerged(ctx)? {
                Ok(RestoreOutcome::Conflicts)
            } else {
                Ok(refused(text))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::RepoLocks;
    use crate::console::Console;
    use crate::task::{Runner, TaskKind};
    use crate::test_support::{in_task, TempRepo};
    use std::sync::{mpsc, Arc};
    use std::time::Duration;
    use tempfile::TempDir;

    const WAIT: Duration = Duration::from_secs(30);

    fn runner() -> Runner {
        Runner::with(
            Arc::<RepoLocks>::default(),
            Arc::<Console>::default(),
            None,
            Duration::from_secs(5),
        )
    }

    fn write(repo: &TempRepo, file: &str, contents: &str) {
        fs::write(repo.path().join(file), contents).unwrap();
    }

    fn read(repo: &TempRepo, file: &str) -> String {
        fs::read_to_string(repo.path().join(file)).unwrap()
    }

    fn with_change() -> TempRepo {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        write(&repo, "a.txt", "local\n");
        repo
    }

    fn save_in(repo: &TempRepo, runner: &Runner) -> Saved {
        in_task(runner, repo.path(), |ctx| save(ctx, "Checkout"))
            .unwrap()
            .unwrap()
    }

    fn stash_count(repo: &TempRepo) -> usize {
        repo.git(&["stash", "list", "--format=%H"]).lines().count()
    }

    #[test]
    fn save_without_changes_saves_nothing() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        write(&repo, "a.txt", "from before\n");
        repo.git(&["stash", "save", "from before"]);

        let saved = in_task(&runner(), repo.path(), |ctx| save(ctx, "Checkout")).unwrap();

        assert_eq!(saved, None);
        assert_eq!(stash_count(&repo), 1);
    }

    #[test]
    fn save_leaves_the_tree_clean_and_records_title_and_sha() {
        let repo = with_change();

        let saved = save_in(&repo, &runner());

        assert_eq!(read(&repo, "a.txt"), "1\n");
        assert!(
            saved
                .message
                .starts_with("Uncommitted changes before Checkout at "),
            "{}",
            saved.message
        );
        assert_eq!(repo.git(&["rev-parse", "stash@{0}"]).trim(), saved.sha);
        assert!(repo
            .git(&["stash", "list", "--format=%gs"])
            .contains(&saved.message));
    }

    #[test]
    fn restore_puts_the_changes_back_and_removes_the_entry() {
        let repo = with_change();
        let runner = runner();
        let saved = save_in(&repo, &runner);

        let outcome = in_task(&runner, repo.path(), move |ctx| restore(ctx, &saved));

        assert_eq!(outcome, Ok(RestoreOutcome::Restored));
        assert_eq!(read(&repo, "a.txt"), "local\n");
        assert_eq!(stash_count(&repo), 0);
    }

    #[test]
    fn another_hand_made_stash_does_not_confuse_it() {
        let repo = with_change();
        let runner = runner();
        let saved = save_in(&repo, &runner);
        write(&repo, "a.txt", "by hand\n");
        repo.git(&["stash", "save", "by hand"]);

        let outcome = in_task(&runner, repo.path(), move |ctx| restore(ctx, &saved));

        assert_eq!(outcome, Ok(RestoreOutcome::Restored));
        assert_eq!(read(&repo, "a.txt"), "local\n");
        assert_eq!(
            repo.git(&["stash", "list", "--format=%s"]).lines().count(),
            1
        );
        assert!(repo
            .git(&["stash", "list", "--format=%s"])
            .contains("by hand"));
    }

    #[test]
    fn entry_that_is_gone_does_not_touch_another() {
        let repo = with_change();
        let runner = runner();
        let saved = save_in(&repo, &runner);
        write(&repo, "a.txt", "by hand\n");
        repo.git(&["stash", "save", "by hand"]);
        repo.git(&["stash", "drop", "stash@{1}"]);

        let outcome = in_task(&runner, repo.path(), move |ctx| restore(ctx, &saved)).unwrap();

        assert_eq!(outcome, RestoreOutcome::Gone);
        assert_eq!(read(&repo, "a.txt"), "1\n");
        assert_eq!(stash_count(&repo), 1);
    }

    #[test]
    fn with_staging_area_what_was_staged_returns_to_the_stage() {
        let repo = with_change();
        repo.git(&["add", "a.txt"]);
        let runner = runner();
        runner.set_staging_area(true);
        let saved = save_in(&repo, &runner);

        in_task(&runner, repo.path(), move |ctx| restore(ctx, &saved)).unwrap();

        assert_eq!(repo.git(&["status", "--porcelain"]), "M  a.txt\n");
    }

    #[test]
    fn without_staging_area_returns_unstaged() {
        let repo = with_change();
        repo.git(&["add", "a.txt"]);
        let runner = runner();
        let saved = save_in(&repo, &runner);

        in_task(&runner, repo.path(), move |ctx| restore(ctx, &saved)).unwrap();

        assert_eq!(repo.git(&["status", "--porcelain"]), " M a.txt\n");
    }

    #[test]
    fn conflict_leaves_the_entry() {
        let repo = with_change();
        let runner = runner();
        let saved = save_in(&repo, &runner);
        repo.commit("a.txt", b"other\n", "two");

        let sha = saved.sha.clone();
        let outcome = in_task(&runner, repo.path(), move |ctx| restore(ctx, &saved));

        assert_eq!(outcome, Ok(RestoreOutcome::Conflicts));
        assert_eq!(repo.git(&["stash", "list", "--format=%H"]).trim(), sha);
        assert!(read(&repo, "a.txt").contains("<<<<<<<"));
    }

    // `pop --index` applies nothing if the stage does not fit, and the work would go unseen. Without
    // `--index` what was on disk comes back, but what was only in the stage does not: the entry is not dropped.
    #[test]
    fn stage_that_does_not_fit_returns_the_disk_and_keeps_the_entry() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n2\n3\n", "one");
        repo.commit("b.txt", b"1\n", "two");
        write(&repo, "a.txt", "1\n2\nstage\n");
        repo.git(&["add", "a.txt"]);
        write(&repo, "a.txt", "disk\n2\nstage\n");
        let runner = runner();
        runner.set_staging_area(true);
        let saved = save_in(&repo, &runner);
        repo.commit("a.txt", b"1\n2\nother\n", "three");

        let sha = saved.sha.clone();
        let outcome = in_task(&runner, repo.path(), move |ctx| restore(ctx, &saved)).unwrap();

        assert_eq!(outcome, RestoreOutcome::Conflicts);
        assert_eq!(repo.git(&["stash", "list", "--format=%H"]).trim(), sha);
        assert!(read(&repo, "a.txt").contains("disk"));
    }

    // The disk merges cleanly with the destination and the stage does not: there is nothing to resolve.
    #[test]
    fn stage_that_does_not_fit_without_conflicts_is_not_called_a_conflict() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n2\n3\n", "one");
        write(&repo, "a.txt", "1\n2\nstage\n");
        repo.git(&["add", "a.txt"]);
        write(&repo, "a.txt", "disk\n2\nother\n");
        let runner = runner();
        runner.set_staging_area(true);
        let saved = save_in(&repo, &runner);
        repo.commit("a.txt", b"1\n2\nother\n", "two");

        let sha = saved.sha.clone();
        let outcome = in_task(&runner, repo.path(), move |ctx| restore(ctx, &saved));

        assert_eq!(outcome, Ok(RestoreOutcome::RestoredWithoutIndex));
        assert_eq!(repo.git(&["ls-files", "--unmerged"]), "");
        assert_eq!(read(&repo, "a.txt"), "disk\n2\nother\n");
        assert_eq!(repo.git(&["stash", "list", "--format=%H"]).trim(), sha);
    }

    fn save_tracked(repo: &TempRepo, runner: &Runner, data: &Path) -> Saved {
        let (path, data) = (repo.path().to_path_buf(), data.to_path_buf());
        in_task(runner, repo.path(), move |ctx| {
            save_with_index_copy(ctx, &path, &data, "Checkout")
        })
        .unwrap()
        .unwrap()
    }

    fn restore_recorded(
        repo: &TempRepo,
        runner: &Runner,
        data: &Path,
        saved: Saved,
    ) -> RestoreOutcome {
        let data = data.to_path_buf();
        in_task(runner, repo.path(), move |ctx| {
            restore_tracked(ctx, &data, &saved)
        })
    }

    #[test]
    fn saved_changes_stay_recorded_with_their_title_until_they_return() {
        let repo = with_change();
        let data = TempDir::new().unwrap();
        let runner = runner();

        let saved = save_tracked(&repo, &runner, data.path());

        let expected = Pending {
            sha: saved.sha.clone(),
            title: "Checkout".to_string(),
            message: saved.message.clone(),
        };
        assert_eq!(pending(repo.path(), data.path()), Ok(vec![expected]));
        let outcome = restore_recorded(&repo, &runner, data.path(), saved);
        assert_eq!(outcome, RestoreOutcome::Restored);
        assert_eq!(pending(repo.path(), data.path()), Ok(Vec::new()));
    }

    #[test]
    fn nothing_is_recorded_if_there_was_nothing_to_save() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        let data = TempDir::new().unwrap();
        let (path, dir) = (repo.path().to_path_buf(), data.path().to_path_buf());

        let saved = in_task(&runner(), repo.path(), move |ctx| {
            save_with_index_copy(ctx, &path, &dir, "Checkout")
        });

        assert_eq!(saved, Ok(None));
        assert_eq!(pending(repo.path(), data.path()), Ok(Vec::new()));
    }

    #[test]
    fn what_does_not_return_stays_recorded() {
        let repo = with_change();
        let data = TempDir::new().unwrap();
        let runner = runner();
        let saved = save_tracked(&repo, &runner, data.path());
        repo.commit("a.txt", b"other\n", "two");

        let outcome = restore_recorded(&repo, &runner, data.path(), saved);

        assert_eq!(outcome, RestoreOutcome::Conflicts);
        assert_eq!(pending(repo.path(), data.path()).unwrap().len(), 1);
    }

    #[test]
    fn popped_by_hand_is_no_longer_recorded() {
        let repo = with_change();
        let data = TempDir::new().unwrap();
        save_tracked(&repo, &runner(), data.path());
        repo.git(&["stash", "pop", "-q"]);

        assert_eq!(pending(repo.path(), data.path()), Ok(Vec::new()));
        assert!(!fs::read_to_string(data.path().join(PENDING_FILE))
            .unwrap()
            .contains("Checkout"));
    }

    #[test]
    fn the_one_that_is_gone_is_forgotten_when_restoring_it() {
        let repo = with_change();
        let data = TempDir::new().unwrap();
        let runner = runner();
        let saved = save_tracked(&repo, &runner, data.path());
        repo.git(&["stash", "drop", "-q"]);

        let outcome = restore_recorded(&repo, &runner, data.path(), saved);

        assert_eq!(outcome, RestoreOutcome::Gone);
        let file = fs::read_to_string(data.path().join(PENDING_FILE)).unwrap();
        assert!(!file.contains("Checkout"));
    }

    #[test]
    fn without_the_stage_nothing_stays_pending_even_if_the_entry_remains() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n2\n3\n", "one");
        write(&repo, "a.txt", "1\n2\nstage\n");
        repo.git(&["add", "a.txt"]);
        write(&repo, "a.txt", "disk\n2\nother\n");
        let data = TempDir::new().unwrap();
        let runner = runner();
        runner.set_staging_area(true);
        let saved = save_tracked(&repo, &runner, data.path());
        repo.commit("a.txt", b"1\n2\nother\n", "two");

        let outcome = restore_recorded(&repo, &runner, data.path(), saved);

        assert_eq!(outcome, RestoreOutcome::RestoredWithoutIndex);
        assert_eq!(stash_count(&repo), 1);
        assert_eq!(pending(repo.path(), data.path()), Ok(Vec::new()));
    }

    #[test]
    fn a_damaged_record_does_not_prevent_saving_and_is_kept() {
        let repo = with_change();
        let data = TempDir::new().unwrap();
        fs::write(data.path().join(PENDING_FILE), b"{ this is not").unwrap();

        save_tracked(&repo, &runner(), data.path());

        assert_eq!(pending(repo.path(), data.path()).unwrap().len(), 1);
        assert!(data.path().join(PENDING_CORRUPT).exists());
    }

    #[test]
    fn cancelling_does_not_leave_saved_changes_unreturned() {
        let repo = with_change();
        let runner = runner();
        let (done, saved) = mpsc::channel();
        let (resume, paused) = mpsc::channel::<()>();
        let (answer, outcome) = mpsc::channel();
        let id = runner.start(
            repo.path(),
            TaskKind::Write,
            "test".to_string(),
            move |ctx| {
                let saved = save(ctx, "Checkout")?.unwrap();
                done.send(()).unwrap();
                paused.recv_timeout(WAIT).unwrap();
                assert_eq!(ctx.check_cancelled(), Err(GitError::Cancelled));
                answer.send(restore(ctx, &saved)).unwrap();
                Ok(())
            },
        );
        saved.recv_timeout(WAIT).unwrap();
        runner.cancel(id);
        resume.send(()).unwrap();

        assert_eq!(
            outcome.recv_timeout(WAIT).unwrap(),
            Ok(RestoreOutcome::Restored)
        );
        assert_eq!(read(&repo, "a.txt"), "local\n");
    }
}
