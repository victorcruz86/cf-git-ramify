// The shelf saver (`GitShelveChangesSaver`, `VcsShelveChangesSaver`): one system list per
// changelist, the shelved files back to HEAD, and later each list unshelved into its changelist.
//
// Never lose work: what each path is (on disk and in the stage) is taken before the changes are
// read, and the files are rolled back only if they are still that, right after the recoverable
// copy. A file written meanwhile would be in neither the shelf nor the copy. Task 9's Shelve uses
// the same check (`AsRead`).
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::SystemTime;

use super::{
    has_unmerged, keep_index_only, RestoreOutcome, SaveFailed, Saved, SavedList, UNMERGED_FIRST,
};
use crate::branches::{self, Repo};
use crate::changelists::{ChangeList, Changelists};
use crate::commands;
use crate::commit;
use crate::git::{self, GitError};
use crate::hunks::{self, LineSelection};
use crate::settings::CleanTree;
use crate::shelf::shelve::{lists_contents, touched, ShelveEntry, SHELVE_REASON};
use crate::shelf::{self, unshelve};
use crate::snapshots;
use crate::status::{self, FileState};
use crate::task::TaskContext;
use crate::worktree_diff::content_hash;

// `GitShelveChangesSaver.restoreStagedWorktree`.
const RESTORE_FROM_HEAD: [&str; 5] = [
    "--literal-pathspecs",
    "restore",
    "--staged",
    "--worktree",
    "--source=HEAD",
];
// Without HEAD every checked path is new: out of the stage, then off the disk. `-f` lets a file
// staged and then edited go too; it only touches the index (`--cached`), and only paths found as
// they were read.
const UNSTAGE_NEW: [&str; 6] = [
    "--literal-pathspecs",
    "rm",
    "--cached",
    "-f",
    "-q",
    "--ignore-unmatch",
];
// `git::git_with_paths`'s batches: the paths of one command stay under the argument limit.
const PATHS_PER_COMMAND: usize = 100;
// `vcs.shelving.changes` and `vcs.unshelving.changes` (V:658-659), the progress text of the saver.
const SHELVING: &str = "Shelving changes…";
const UNSHELVING: &str = "Unshelving changes…";

//
// What a path was when it was read
//

// Each path as it was on disk (kind, executable bit, content hash or link target) and in the stage
// (mode, blob, stage).
pub(crate) struct AsRead(BTreeMap<String, String>);

fn on_disk(repo: &Path, path: &str) -> io::Result<String> {
    let full = repo.join(path);
    let meta = match full.symlink_metadata() {
        Ok(meta) => meta,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok("absent".to_string());
        }
        Err(error) => return Err(error),
    };
    if meta.file_type().is_symlink() {
        return Ok(format!("link {}", fs::read_link(&full)?.display()));
    }
    if meta.is_dir() {
        return Ok("folder".to_string());
    }
    let executable = meta.permissions().mode() & 0o111 != 0;
    Ok(format!(
        "file {executable} {}",
        content_hash(&fs::read(&full)?)
    ))
}

// `ls-files -s -z`: `mode blob stage<TAB>path`, by path.
fn in_stage(repo: &Path, paths: &[String]) -> Result<BTreeMap<String, String>, GitError> {
    let output = git::git_with_paths(repo, false, &["ls-files", "-s", "-z"], paths)?;
    let mut staged: BTreeMap<String, String> = BTreeMap::new();
    for record in output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let record = String::from_utf8_lossy(record);
        if let Some((entry, path)) = record.split_once('\t') {
            let known = staged.entry(path.to_string()).or_default();
            known.push_str(entry);
            known.push(' ');
        }
    }
    Ok(staged)
}

// A path that may not be touched is not taken: it is never stored, so never rolled back.
fn states(repo: &Path, paths: &[String]) -> Result<BTreeMap<String, String>, GitError> {
    let paths: Vec<String> = paths
        .iter()
        .filter(|path| snapshots::check_path(repo, path).is_ok())
        .cloned()
        .collect();
    let paths = &paths;
    let staged = in_stage(repo, paths)?;
    let mut states = BTreeMap::new();
    for path in paths {
        let disk = on_disk(repo, path).map_err(git::io_error)?;
        let stage = staged.get(path).map_or("", String::as_str);
        states.insert(path.clone(), format!("{disk} | {stage}"));
    }
    Ok(states)
}

impl AsRead {
    // Before the changes are read.
    pub(crate) fn take(repo: &Path, paths: &[String]) -> Result<Self, GitError> {
        states(repo, paths).map(Self)
    }

    // For what rolls nothing back: no path was taken.
    pub(crate) fn nothing() -> Self {
        Self(BTreeMap::new())
    }

    // Which of `paths` are no longer what they were, or were not taken. A path `check_path` refuses
    // is in neither reading and compares as unchanged: the copy refuses it first.
    pub(crate) fn changed(
        &self,
        repo: &Path,
        paths: &[String],
    ) -> Result<BTreeSet<String>, GitError> {
        let now = states(repo, paths)?;
        Ok(paths
            .iter()
            .filter(|path| self.0.get(*path) != now.get(*path))
            .cloned()
            .collect())
    }
}

// rama's own: what was written while it was shelved is not rolled back.
pub(crate) fn changed_while_shelving(paths: &BTreeSet<String>) -> String {
    let names: Vec<String> = paths.iter().map(|path| format!("\"{path}\"")).collect();
    let (verb, left) = match paths.len() {
        1 => ("it was", "was left as it is"),
        _ => ("they were", "were left as they are"),
    };
    format!(
        "{} changed while {verb} being shelved and {left}.",
        names.join(", ")
    )
}

//
// Saving
//

// The changes of one changelist, or of all of them with the staging area.
struct Group {
    changelist: Option<ChangeList>,
    entries: Vec<ShelveEntry>,
}

// The lists other than `own` some of the file's hunks were moved to, by hunk; empty when the file
// is not split (or its hunks cannot be read: then it goes whole to its list).
fn split_parts(
    repo: &Path,
    lists: &Changelists,
    path: &str,
    own: &str,
) -> BTreeMap<String, Vec<String>> {
    let exists = |id: &String| lists.lists.iter().any(|list| list.id == *id);
    let Some(known) = lists.hunk_assignments.get(path) else {
        return BTreeMap::new();
    };
    if known.values().all(|id| id == own || !exists(id)) {
        return BTreeMap::new();
    }
    let Ok(hunks) = hunks::file_hunks(repo, path) else {
        return BTreeMap::new();
    };
    let mut parts: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for hunk in hunks {
        let list = known
            .get(&hunk.id)
            .filter(|id| exists(id))
            .cloned()
            .unwrap_or_else(|| own.to_string());
        parts.entry(list).or_default().push(hunk.id);
    }
    parts
}

// `VcsShelveChangesSaver.save`: the versioned changes (`getAllChanges`, untracked files are not
// among them), per changelist in the lists' order. A file goes whole to its list, or by its hunks
// to each list that has some (`ChangeListChange`). With the staging area there are no changelists:
// one group.
fn changelist_groups(repo: &Path, data: &Path, staging: bool) -> Result<Vec<Group>, GitError> {
    let changes: Vec<status::FileChange> = status::read(repo, false)?
        .into_iter()
        .filter(|change| {
            !matches!(
                change.state,
                FileState::Untracked | FileState::Ignored | FileState::Unmerged
            )
        })
        .collect();
    let entry = |change: &status::FileChange, selection| ShelveEntry {
        path: change.path.clone(),
        old_path: change.old_path.clone(),
        selection,
    };
    if changes.is_empty() {
        return Ok(Vec::new());
    }
    if staging {
        let entries = changes.iter().map(|change| entry(change, None)).collect();
        return Ok(vec![Group {
            changelist: None,
            entries,
        }]);
    }
    let lists = Changelists::load(data);
    let mut by_list: BTreeMap<String, Vec<ShelveEntry>> = BTreeMap::new();
    for change in &changes {
        let assigned = lists.list_for(&change.path);
        let own = match lists.lists.iter().any(|list| list.id == assigned) {
            true => assigned.to_string(),
            false => lists.active.clone(),
        };
        let parts = split_parts(repo, &lists, &change.path, &own);
        if parts.len() > 1 {
            for (id, hunks) in parts {
                let selection = hunks
                    .into_iter()
                    .map(|hunk| LineSelection { hunk, lines: None })
                    .collect();
                by_list
                    .entry(id)
                    .or_default()
                    .push(entry(change, Some(selection)));
            }
        } else {
            let id = parts.into_keys().next().unwrap_or(own);
            by_list.entry(id).or_default().push(entry(change, None));
        }
    }
    Ok(lists
        .lists
        .iter()
        .filter_map(|list| {
            by_list.remove(&list.id).map(|entries| Group {
                changelist: Some(list.clone()),
                entries,
            })
        })
        .collect())
}

// `GitShelveChangesSaver.rollbackChanges`: the checked paths back to HEAD in the stage and on disk.
// Each batch waits for an `index.lock` another git holds for a moment (`git::retrying`). What was
// only in the stage is in the copy of `keep_index_only`.
//
// Without HEAD IntelliJ runs `reset --hard`, which also takes what was staged after the check.
// rama only takes the checked paths out of the stage and off the disk: they are all new.
fn roll_back(ctx: &TaskContext, repo: &Path, paths: &[String]) -> Result<(), GitError> {
    let head = git::head_exists(repo)?;
    let command: &[&str] = match head {
        true => &RESTORE_FROM_HEAD,
        false => &UNSTAGE_NEW,
    };
    for chunk in paths.chunks(PATHS_PER_COMMAND) {
        let mut args: Vec<&str> = command.to_vec();
        args.push("--");
        args.extend(chunk.iter().map(String::as_str));
        git::retrying(|| ctx.git_to_the_end(&args))?;
    }
    if !head {
        for path in paths {
            let full = repo.join(path);
            let file = full.symlink_metadata().is_ok_and(|meta| !meta.is_dir());
            if file {
                fs::remove_file(&full).map_err(git::io_error)?;
            }
        }
    }
    Ok(())
}

// rama's own: the rollback after shelving failed, and the changes are in both places.
fn kept_in_shelf(lists: &[SavedList]) -> String {
    let names: Vec<String> = lists
        .iter()
        .map(|list| format!("\"{}\"", list.description))
        .collect();
    format!(
        "The changes were saved to the shelf as {}, but not all of them were rolled back.",
        names.join(", ")
    )
}

// What was made goes to Recently Deleted: nothing was rolled back, the changes are still on disk.
fn withdraw(data: &Path, lists: &[SavedList]) {
    for list in lists {
        let _ = shelf::delete(data, &list.name, SystemTime::now());
    }
}

// `VcsShelveChangesSaver.save`: one system list (`toDelete`) per changelist with changes, named
// `<message> [<list>]` (`createSystemShelvedChangeListName`), or `<message>` with the staging area;
// then the shelved files go back to HEAD. `None` if there was nothing to shelve.
pub(super) fn shelve_as(
    ctx: &TaskContext,
    repo: &Path,
    data: &Path,
    title: &str,
    message: &str,
) -> Result<Option<Saved>, SaveFailed> {
    shelve_watching(ctx, repo, data, title, message, &mut |_| {})
}

// When `shelve_watching` lets the tests act as another program: once the lists are stored, right
// before the copy, and once the copy is taken, right before the check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Moment {
    Stored,
    Copied,
}

// `watch` runs at each `Moment`: the tests
// write there as another program.
fn shelve_watching(
    ctx: &TaskContext,
    repo: &Path,
    data: &Path,
    title: &str,
    message: &str,
    watch: &mut dyn FnMut(Moment),
) -> Result<Option<Saved>, SaveFailed> {
    ctx.check_cancelled()?;
    ctx.set_text(SHELVING);
    let saved = shelve_lists(ctx, repo, data, title, message, watch);
    ctx.set_text("");
    saved
}

fn shelve_lists(
    ctx: &TaskContext,
    repo: &Path,
    data: &Path,
    title: &str,
    message: &str,
    watch: &mut dyn FnMut(Moment),
) -> Result<Option<Saved>, SaveFailed> {
    let groups = changelist_groups(repo, data, ctx.staging_area())?;
    let as_read = AsRead::take(
        repo,
        &touched(groups.iter().flat_map(|group| &group.entries)),
    )?;
    let entries: Vec<&[ShelveEntry]> = groups.iter().map(|group| &group.entries[..]).collect();
    let mut read = Vec::new();
    for contents in lists_contents(repo, &entries) {
        read.push(contents?);
    }
    let stored: Vec<&ShelveEntry> = read
        .iter()
        .flat_map(|contents| contents.stored.iter().copied())
        .collect();
    if stored.is_empty() {
        return Ok(None);
    }
    keep_index_only(repo, data, title)?;
    let mut lists = Vec::new();
    for (group, contents) in groups.iter().zip(&read) {
        if contents.stored.is_empty() {
            continue;
        }
        let description = match &group.changelist {
            Some(list) => format!("{message} [{}]", list.name),
            None => message.to_string(),
        };
        let created = shelf::create(
            data,
            repo,
            &description,
            &contents.patch,
            &contents.binaries,
            true,
        );
        match created {
            Ok(list) => lists.push(SavedList {
                name: list.name,
                description: list.description,
                changelist: group.changelist.as_ref().map(|list| list.id.clone()),
            }),
            Err(error) => {
                withdraw(data, &lists);
                return Err(GitError::Io(error.to_string()).into());
            }
        }
    }
    watch(Moment::Stored);
    let paths = touched(stored);
    if let Err(error) = snapshots::create(data, repo, &paths, SHELVE_REASON) {
        withdraw(data, &lists);
        return Err(super::backup_failed(error).into());
    }
    watch(Moment::Copied);
    let changed = match as_read.changed(repo, &paths) {
        Ok(changed) => changed,
        Err(error) => {
            withdraw(data, &lists);
            return Err(error.into());
        }
    };
    if !changed.is_empty() {
        withdraw(data, &lists);
        let text = format!("{} Nothing was shelved.", changed_while_shelving(&changed));
        return Err(GitError::Io(text).into());
    }
    let rolled = roll_back(ctx, repo, &paths);
    let saved = Saved {
        kind: CleanTree::Shelve,
        sha: String::new(),
        message: message.to_string(),
        lists,
    };
    match rolled {
        Ok(()) => Ok(Some(saved)),
        Err(error) => {
            let text = format!(
                "{}\n{}",
                branches::text_of(error),
                kept_in_shelf(&saved.lists)
            );
            Err(SaveFailed {
                error: GitError::Io(text),
                saved: Some(Box::new(saved)),
            })
        }
    }
}

//
// Restoring
//

// The ids of the file's hunks now; none if they cannot be read.
fn hunk_ids(repo: &Path, path: &str) -> BTreeSet<String> {
    hunks::file_hunks(repo, path)
        .map(|hunks| hunks.into_iter().map(|hunk| hunk.id).collect())
        .unwrap_or_default()
}

// A file shelved by parts goes back split as it was: whole to the first list that gave part of it
// back, then each later list takes the hunks its unshelve added.
type Restored = BTreeMap<String, Vec<(String, BTreeSet<String>)>>;

fn split_back(repo: &Path, data: &Path, restored: &Restored) {
    let split: Vec<_> = restored
        .iter()
        .filter(|(_, parts)| parts.len() > 1)
        .collect();
    if split.is_empty() {
        return;
    }
    let current: BTreeMap<&String, Vec<String>> = split
        .iter()
        .map(|(path, _)| (*path, hunk_ids(repo, path).into_iter().collect()))
        .collect();
    let _ = commands::edit_changelists(data, |lists| {
        for (path, parts) in &split {
            let now = &current[path];
            let _ = lists.move_paths(&[path.to_string()], &parts[0].0);
            lists.reconcile_hunks(path, now);
            for (id, added) in &parts[1..] {
                let hunks: Vec<String> = now
                    .iter()
                    .filter(|id| added.contains(*id))
                    .cloned()
                    .collect();
                if !hunks.is_empty() {
                    let _ = lists.move_hunks(path, &hunks, id);
                }
            }
        }
        Ok(())
    });
}

// `VcsShelveChangesSaver.load`: each list still in the shelf is unshelved into the changelist it
// came from, and recycled once applied (a system list then counts as deleted). What does not fit is
// not written and stays in its list: a conflict, a change that does not apply, a new file whose
// place was taken. Not with a merge or a rebase half-done, as the stash.
pub(super) fn unshelve_saved(
    ctx: &TaskContext,
    data: &Path,
    saved: &Saved,
) -> Result<RestoreOutcome, GitError> {
    ctx.set_text(UNSHELVING);
    let outcome = unshelve_lists(ctx, data, saved);
    ctx.set_text("");
    outcome
}

fn unshelve_lists(
    ctx: &TaskContext,
    data: &Path,
    saved: &Saved,
) -> Result<RestoreOutcome, GitError> {
    let repo = ctx.repo();
    let store = shelf::list(data);
    let alive: Vec<&SavedList> = saved
        .lists
        .iter()
        .filter(|saved| {
            store
                .iter()
                .any(|list| list.name == saved.name && !list.recycled && !list.deleted)
        })
        .collect();
    if alive.is_empty() {
        return Ok(RestoreOutcome::Gone);
    }
    if commit::merge_in_progress(repo) || commit::rebase_in_progress(repo) || has_unmerged(ctx)? {
        return Ok(RestoreOutcome::Failed(UNMERGED_FIRST.to_string()));
    }
    let target = Repo {
        ctx,
        path: repo,
        data,
    };
    let mut conflicts = false;
    let mut failures = Vec::new();
    let mut blocked = Vec::new();
    let mut seen: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut restored = Restored::new();
    for list in alive {
        let into = list.changelist.as_deref().filter(|_| !ctx.staging_area());
        let now = SystemTime::now();
        let unshelved = match unshelve::unshelve(&target, &list.name, into, now) {
            Ok(unshelved) => unshelved,
            Err(error) => {
                failures.push(branches::text_of(error));
                continue;
            }
        };
        conflicts |= !unshelved.applied.conflicts.is_empty();
        if !unshelved.applied.failed.is_empty() {
            failures.extend(unshelved.applied.messages.iter().cloned());
        }
        failures.extend(unshelved.binaries_failed.iter().cloned());
        blocked.extend(unshelved.skipped.iter().cloned());
        if let Some(id) = into {
            for path in &unshelved.written {
                let now = hunk_ids(repo, path);
                let before = seen.insert(path.clone(), now.clone()).unwrap_or_default();
                let added = now.difference(&before).cloned().collect();
                let parts = restored.entry(path.clone()).or_default();
                parts.push((id.to_string(), added));
            }
        }
    }
    split_back(repo, data, &restored);
    Ok(if conflicts {
        RestoreOutcome::Conflicts
    } else if !failures.is_empty() {
        RestoreOutcome::Failed(failures.join("\n"))
    } else if !blocked.is_empty() {
        RestoreOutcome::RestoreBlocked { files: blocked }
    } else {
        RestoreOutcome::Restored
    })
}

#[cfg(test)]
mod tests {
    use super::super::{pending, restore_tracked, saver};
    use super::*;
    use crate::commands::RepoLocks;
    use crate::console::Console;
    use crate::settings;
    use crate::task::Runner;
    use crate::test_support::{in_task, TempRepo};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tempfile::TempDir;

    // An empty data folder has the default, Shelve.
    fn runner() -> Runner {
        runner_with(None)
    }

    fn runner_with(binary: Option<std::path::PathBuf>) -> Runner {
        Runner::with(
            Arc::<RepoLocks>::default(),
            Arc::<Console>::default(),
            binary,
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

    fn stash_count(repo: &TempRepo) -> usize {
        repo.git(&["stash", "list", "--format=%H"]).lines().count()
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

    fn shelve_local(repo: &TempRepo, runner: &Runner, data: &Path) -> Option<Saved> {
        let (path, data) = (repo.path().to_path_buf(), data.to_path_buf());
        in_task(runner, repo.path(), move |ctx| {
            super::super::save_local(ctx, &path, &data, "Update")
        })
        .unwrap()
    }

    fn other_list(data: &Path, paths: &[&str]) -> String {
        let mut lists = Changelists::load(data);
        let other = lists.create("Other").unwrap();
        let paths: Vec<String> = paths.iter().map(|path| path.to_string()).collect();
        lists.move_paths(&paths, &other).unwrap();
        lists.save(data).unwrap();
        other
    }

    // What a refresh does while the tree is clean: the lists forget the files.
    fn forget_files(data: &Path) {
        let mut lists = Changelists::load(data);
        lists.reconcile(&[]);
        lists.save(data).unwrap();
    }

    fn alive(data: &Path) -> Vec<shelf::ShelvedList> {
        shelf::list(data)
            .into_iter()
            .filter(|list| !list.recycled && !list.deleted)
            .collect()
    }

    #[test]
    fn shelve_saves_one_list_per_changelist_and_restores_into_it() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        repo.commit("b.txt", b"b\n", "two");
        write(&repo, "a.txt", "local a\n");
        write(&repo, "b.txt", "local b\n");
        write(&repo, "c.txt", "new\n");
        repo.git(&["add", "c.txt"]);
        write(&repo, "u.txt", "untracked\n");
        let data = TempDir::new().unwrap();
        let other = other_list(data.path(), &["b.txt"]);
        let runner = runner();

        let saved = shelve_local(&repo, &runner, data.path()).unwrap();

        assert_eq!(saved.kind, CleanTree::Shelve);
        assert!(saved
            .message
            .starts_with("Uncommitted changes before Update at "));
        let names: Vec<&str> = saved
            .lists
            .iter()
            .map(|list| list.description.as_str())
            .collect();
        assert_eq!(
            names,
            [
                format!("{} [Changes]", saved.message),
                format!("{} [Other]", saved.message)
            ]
        );
        assert_eq!(saved.lists[1].changelist.as_deref(), Some(other.as_str()));
        let stored = alive(data.path());
        assert_eq!(stored.len(), 2);
        assert!(stored.iter().all(|list| list.to_delete));
        assert_eq!(repo.git(&["status", "--porcelain"]), "?? u.txt\n");
        assert_eq!(stash_count(&repo), 0);
        assert!(snapshots::list(data.path())
            .iter()
            .any(|copy| copy.reason == "Shelve changes"));
        forget_files(data.path());

        let outcome = restore_recorded(&repo, &runner, data.path(), saved);

        assert_eq!(outcome, RestoreOutcome::Restored);
        assert_eq!(read(&repo, "a.txt"), "local a\n");
        assert_eq!(read(&repo, "b.txt"), "local b\n");
        assert_eq!(read(&repo, "c.txt"), "new\n");
        assert_eq!(repo.git(&["status", "--porcelain", "c.txt"]), "A  c.txt\n");
        let lists = Changelists::load(data.path());
        assert_eq!(lists.list_for("b.txt"), other);
        assert_eq!(lists.list_for("a.txt"), lists.lists[0].id);
        assert_eq!(alive(data.path()), []);
        assert!(shelf::list(data.path()).iter().all(|list| list.deleted));
        assert_eq!(pending(repo.path(), data.path()), Ok(Vec::new()));
    }

    #[test]
    fn with_the_staging_area_one_list_takes_the_message() {
        let repo = with_change();
        let data = TempDir::new().unwrap();
        let runner = runner();
        runner.set_staging_area(true);

        let saved = shelve_local(&repo, &runner, data.path()).unwrap();

        assert_eq!(saved.lists.len(), 1);
        assert_eq!(saved.lists[0].description, saved.message);
        assert_eq!(saved.lists[0].changelist, None);
        assert_eq!(read(&repo, "a.txt"), "1\n");
        let outcome = restore_recorded(&repo, &runner, data.path(), saved);
        assert_eq!(outcome, RestoreOutcome::Restored);
        assert_eq!(read(&repo, "a.txt"), "local\n");
    }

    #[test]
    fn index_only_content_is_kept_in_a_snapshot() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        write(&repo, "a.txt", "staged\n");
        repo.git(&["add", "a.txt"]);
        write(&repo, "a.txt", "disk\n");
        let data = TempDir::new().unwrap();
        let runner = runner();
        // The shelf never keeps the stage, with the staging area or without it.
        runner.set_staging_area(true);

        let saved = shelve_local(&repo, &runner, data.path()).unwrap();

        let copy = snapshots::list(data.path())
            .into_iter()
            .find(|copy| copy.reason == "Update")
            .expect("a copy of the stage");
        let staged = snapshots::stored(data.path(), &copy.id, "a.txt", true).unwrap();
        assert_eq!(fs::read_to_string(staged).unwrap(), "staged\n");
        assert_eq!(repo.git(&["status", "--porcelain"]), "");
        restore_recorded(&repo, &runner, data.path(), saved);
        assert_eq!(read(&repo, "a.txt"), "disk\n");
    }

    #[test]
    fn a_conflict_stays_in_the_shelf_and_nothing_is_overwritten() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n2\n3\n", "one");
        write(&repo, "a.txt", "local\n2\n3\n");
        let data = TempDir::new().unwrap();
        let runner = runner();
        let saved = shelve_local(&repo, &runner, data.path()).unwrap();
        repo.commit("a.txt", b"remote\n2\n3\n", "two");

        let outcome = restore_recorded(&repo, &runner, data.path(), saved.clone());

        assert_eq!(outcome, RestoreOutcome::Conflicts);
        assert_eq!(read(&repo, "a.txt"), "remote\n2\n3\n");
        let kept = alive(data.path());
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].name, saved.lists[0].name);
        let patch = shelf::load_patch(data.path(), &kept[0].name).unwrap();
        assert_eq!(patch.files.len(), 1);
    }

    #[test]
    fn what_merges_with_the_new_head_comes_back() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n2\n3\n4\n5\n6\n", "one");
        write(&repo, "a.txt", "local\n2\n3\n4\n5\n6\n");
        let data = TempDir::new().unwrap();
        let runner = runner();
        let saved = shelve_local(&repo, &runner, data.path()).unwrap();
        repo.commit("a.txt", b"1\n2\n3\n4\n5\nremote\n", "two");

        let outcome = restore_recorded(&repo, &runner, data.path(), saved);

        assert_eq!(outcome, RestoreOutcome::Restored);
        assert_eq!(read(&repo, "a.txt"), "local\n2\n3\n4\n5\nremote\n");
    }

    #[test]
    fn a_file_split_across_lists_goes_back_split() {
        let repo = TempRepo::new();
        let lines: String = (1..=12).map(|n| format!("{n}\n")).collect();
        repo.commit("a.txt", lines.as_bytes(), "one");
        write(
            &repo,
            "a.txt",
            &lines
                .replace("1\n2\n", "top\n2\n")
                .replace("12\n", "bottom\n"),
        );
        let data = TempDir::new().unwrap();
        let other = other_list(data.path(), &[]);
        let ids: Vec<String> = hunks::file_hunks(repo.path(), "a.txt")
            .unwrap()
            .into_iter()
            .map(|hunk| hunk.id)
            .collect();
        assert_eq!(ids.len(), 2);
        let moved = &ids[1..];
        crate::commands::move_hunks_in(repo.path(), data.path(), "a.txt", moved, &other).unwrap();
        let runner = runner();

        let saved = shelve_local(&repo, &runner, data.path()).unwrap();

        assert_eq!(saved.lists.len(), 2);
        for list in &saved.lists {
            let patch = shelf::load_patch(data.path(), &list.name).unwrap();
            assert_eq!(patch.files.len(), 1, "{}", list.description);
        }
        assert_eq!(read(&repo, "a.txt"), lines);
        forget_files(data.path());
        let outcome = restore_recorded(&repo, &runner, data.path(), saved);
        assert_eq!(outcome, RestoreOutcome::Restored);
        assert!(read(&repo, "a.txt").starts_with("top\n"));
        assert!(read(&repo, "a.txt").ends_with("bottom\n"));
        let lists = Changelists::load(data.path());
        assert_eq!(lists.hunks_in("a.txt", &other), ids[1..]);
        assert_eq!(lists.hunks_in("a.txt", &lists.lists[0].id), ids[..1]);
    }

    #[test]
    fn binaries_renames_and_deletions_come_back() {
        let repo = TempRepo::new();
        repo.commit("bin.dat", b"\x00\x01old", "one");
        repo.commit("r.txt", b"rename me\n", "two");
        repo.commit("d.txt", b"delete me\n", "three");
        fs::write(repo.path().join("bin.dat"), b"\x00\x02new").unwrap();
        fs::write(repo.path().join("new.dat"), b"\x00\x03added").unwrap();
        repo.git(&["add", "new.dat"]);
        repo.git(&["mv", "r.txt", "s.txt"]);
        fs::remove_file(repo.path().join("d.txt")).unwrap();
        let before = repo.git(&["status", "--porcelain"]);
        let data = TempDir::new().unwrap();
        let runner = runner();

        let saved = shelve_local(&repo, &runner, data.path()).unwrap();

        assert_eq!(repo.git(&["status", "--porcelain"]), "");
        assert!(!repo.path().join("new.dat").exists());
        let outcome = restore_recorded(&repo, &runner, data.path(), saved);
        assert_eq!(outcome, RestoreOutcome::Restored);
        assert_eq!(
            fs::read(repo.path().join("bin.dat")).unwrap(),
            b"\x00\x02new"
        );
        assert_eq!(
            fs::read(repo.path().join("new.dat")).unwrap(),
            b"\x00\x03added"
        );
        assert_eq!(read(&repo, "s.txt"), "rename me\n");
        assert!(!repo.path().join("r.txt").exists());
        assert!(!repo.path().join("d.txt").exists());
        // A deletion comes back as `git rm` does it, as an applied patch's: in the stage too.
        let now = repo.git(&["status", "--porcelain"]);
        let mut now: Vec<String> = now.lines().map(String::from).collect();
        let was = before.replace(" D d.txt", "D  d.txt");
        let mut was: Vec<String> = was.lines().map(String::from).collect();
        now.sort();
        was.sort();
        assert_eq!(now, was);
    }

    #[test]
    fn nothing_to_shelve_saves_nothing() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        write(&repo, "u.txt", "untracked\n");
        let data = TempDir::new().unwrap();

        assert_eq!(shelve_local(&repo, &runner(), data.path()), None);
        assert_eq!(shelf::list(data.path()), []);
        assert_eq!(read(&repo, "u.txt"), "untracked\n");
    }

    #[test]
    fn a_list_deleted_meanwhile_is_gone() {
        let repo = with_change();
        let data = TempDir::new().unwrap();
        let runner = runner();
        let saved = shelve_local(&repo, &runner, data.path()).unwrap();
        shelf::delete(data.path(), &saved.lists[0].name, SystemTime::now()).unwrap();

        let outcome = restore_recorded(&repo, &runner, data.path(), saved);

        assert_eq!(outcome, RestoreOutcome::Gone);
        assert_eq!(read(&repo, "a.txt"), "1\n");
    }

    // `act` runs once, at `at`.
    fn shelve_at(
        repo: &TempRepo,
        runner: &Runner,
        data: &Path,
        at: Moment,
        act: impl FnMut() + Send + 'static,
    ) -> Result<Option<Saved>, GitError> {
        let (path, data) = (repo.path().to_path_buf(), data.to_path_buf());
        let mut act = act;
        let mut watch = move |moment: Moment| {
            if moment == at {
                act();
            }
        };
        in_task(runner, repo.path(), move |ctx| {
            shelve_watching(ctx, &path, &data, "Update", "Before Update", &mut watch)
                .map_err(|failed| failed.error)
        })
    }

    fn shelve_with(
        repo: &TempRepo,
        runner: &Runner,
        data: &Path,
        before_copy: impl FnMut() + Send + 'static,
    ) -> Result<Option<Saved>, GitError> {
        shelve_at(repo, runner, data, Moment::Stored, before_copy)
    }

    fn two_changes() -> TempRepo {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        repo.commit("b.txt", b"b\n", "two");
        write(&repo, "a.txt", "local a\n");
        write(&repo, "b.txt", "local b\n");
        repo
    }

    // Review Focus 1 on the saver: a file written after it was read is in neither the shelf nor
    // the copy, so nothing is rolled back, and the lists go to Recently Deleted.
    #[test]
    fn a_file_written_between_shelving_and_rollback_survives() {
        let repo = two_changes();
        let data = TempDir::new().unwrap();
        let target = repo.path().join("a.txt");

        let saved = shelve_with(&repo, &runner(), data.path(), move || {
            fs::write(&target, "devin\n").unwrap();
        });

        let error = branches::text_of(saved.unwrap_err());
        assert!(
            error
                .starts_with("\"a.txt\" changed while it was being shelved and was left as it is."),
            "{error}"
        );
        assert_eq!(read(&repo, "a.txt"), "devin\n");
        assert_eq!(read(&repo, "b.txt"), "local b\n");
        assert_eq!(alive(data.path()), []);
        assert!(shelf::list(data.path()).iter().all(|list| list.deleted));
    }

    #[test]
    fn the_copy_is_taken_right_before_the_rollback() {
        let repo = two_changes();
        let data = TempDir::new().unwrap();
        let (dir, copies) = (data.path().to_path_buf(), Arc::new(Mutex::new(0)));
        let seen = copies.clone();

        let saved = shelve_with(&repo, &runner(), data.path(), move || {
            let count = snapshots::list(&dir)
                .iter()
                .filter(|copy| copy.reason == SHELVE_REASON)
                .count();
            *seen.lock().unwrap() = count;
        });

        assert!(saved.unwrap().is_some());
        assert_eq!(*copies.lock().unwrap(), 0);
        let copy = snapshots::list(data.path())
            .into_iter()
            .find(|copy| copy.reason == SHELVE_REASON)
            .unwrap();
        let stored = snapshots::stored(data.path(), &copy.id, "a.txt", false).unwrap();
        assert_eq!(fs::read_to_string(stored).unwrap(), "local a\n");
    }

    // A path is told apart by its content, not by its size and time.
    #[test]
    fn as_read_sees_a_same_size_write_and_a_stage_change() {
        let repo = two_changes();
        let paths = vec!["a.txt".to_string(), "b.txt".to_string()];
        let read_before = AsRead::take(repo.path(), &paths).unwrap();
        write(&repo, "a.txt", "local A\n");
        repo.git(&["add", "b.txt"]);

        let changed = read_before.changed(repo.path(), &paths).unwrap();

        assert_eq!(
            changed,
            BTreeSet::from(["a.txt".to_string(), "b.txt".to_string()])
        );
        let again = AsRead::take(repo.path(), &paths).unwrap();
        assert_eq!(again.changed(repo.path(), &paths).unwrap(), BTreeSet::new());
    }

    // rama's own: without HEAD only the checked paths go back (IntelliJ resets the whole tree).
    #[test]
    fn without_head_only_the_checked_paths_go_back() {
        let repo = TempRepo::new();
        write(&repo, "a.txt", "first\n");
        repo.git(&["add", "a.txt"]);
        let data = TempDir::new().unwrap();
        let runner = runner();

        let saved = shelve_local(&repo, &runner, data.path()).unwrap();

        assert!(!repo.path().join("a.txt").exists());
        assert_eq!(repo.git(&["status", "--porcelain"]), "");
        let outcome = restore_recorded(&repo, &runner, data.path(), saved);
        assert_eq!(outcome, RestoreOutcome::Restored);
        assert_eq!(read(&repo, "a.txt"), "first\n");
        assert_eq!(repo.git(&["status", "--porcelain"]), "A  a.txt\n");
    }

    // P24: without HEAD a file staged and then edited also goes back: `git rm --cached` alone
    // refuses a file whose stage differs from both HEAD and the disk.
    #[test]
    fn without_head_a_file_staged_then_edited_goes_back() {
        let repo = TempRepo::new();
        write(&repo, "a.txt", "staged\n");
        repo.git(&["add", "a.txt"]);
        write(&repo, "a.txt", "edited\n");
        let data = TempDir::new().unwrap();
        let runner = runner();

        let saved = shelve_local(&repo, &runner, data.path()).unwrap();

        assert!(!repo.path().join("a.txt").exists());
        assert_eq!(repo.git(&["status", "--porcelain"]), "");
        let outcome = restore_recorded(&repo, &runner, data.path(), saved);
        assert_eq!(outcome, RestoreOutcome::Restored);
        assert_eq!(read(&repo, "a.txt"), "edited\n");
    }

    // A file staged after the changes were read is in neither the shelf nor the copy: it stays.
    #[test]
    fn without_head_a_file_added_meanwhile_is_not_rolled_back() {
        let repo = TempRepo::new();
        write(&repo, "a.txt", "first\n");
        repo.git(&["add", "a.txt"]);
        let data = TempDir::new().unwrap();
        let root = repo.path().to_path_buf();

        let saved = shelve_with(&repo, &runner(), data.path(), move || {
            fs::write(root.join("devin.txt"), "devin\n").unwrap();
            let added = std::process::Command::new("git")
                .current_dir(&root)
                .args(["add", "devin.txt"])
                .status()
                .unwrap();
            assert!(added.success());
        });

        assert!(saved.unwrap().is_some());
        assert!(!repo.path().join("a.txt").exists());
        assert_eq!(read(&repo, "devin.txt"), "devin\n");
        assert_eq!(repo.git(&["status", "--porcelain"]), "A  devin.txt\n");
    }

    // Review Focus 1: when the check itself fails, nothing was rolled back and the lists go to
    // Recently Deleted, as for every failure after they were stored.
    #[test]
    fn a_failed_check_withdraws_the_lists() {
        let repo = two_changes();
        let data = TempDir::new().unwrap();
        let target = repo.path().join("a.txt");

        let saved = shelve_at(&repo, &runner(), data.path(), Moment::Copied, move || {
            fs::set_permissions(&target, fs::Permissions::from_mode(0o000)).unwrap();
        });
        let file = repo.path().join("a.txt");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();

        assert!(saved.is_err());
        assert_eq!(read(&repo, "a.txt"), "local a\n");
        assert_eq!(read(&repo, "b.txt"), "local b\n");
        assert_eq!(alive(data.path()), []);
        assert!(shelf::list(data.path()).iter().all(|list| list.deleted));
    }

    // `GitHandler`'s wait for `index.lock`: another program's git that holds the index for a
    // moment does not fail the rollback halfway.
    #[test]
    fn a_held_index_lock_is_waited_for() {
        let scripts = TempDir::new().unwrap();
        let script = scripts.path().join("git");
        let mark = scripts.path().join("mark");
        let text = format!(
            concat!(
                "#!/bin/sh\ncase \" $* \" in\n",
                "*\" restore \"*) if [ ! -e '{mark}' ]; then : > '{mark}'; ",
                "echo \"fatal: Unable to create '$PWD/.git/index.lock': File exists.\" >&2; ",
                "exit 128; fi; exec git \"$@\" ;;\n",
                "*) exec git \"$@\" ;;\nesac\n"
            ),
            mark = mark.display()
        );
        fs::write(&script, text).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let repo = two_changes();
        let data = TempDir::new().unwrap();

        let saved = shelve_with(&repo, &runner_with(Some(script)), data.path(), || {});

        assert!(saved.unwrap().is_some());
        assert!(mark.exists());
        assert_eq!(repo.git(&["status", "--porcelain"]), "");
    }

    #[test]
    fn a_failed_rollback_keeps_the_lists_and_says_where() {
        let scripts = TempDir::new().unwrap();
        let script = scripts.path().join("git");
        let text = concat!(
            "#!/bin/sh\ncase \" $* \" in\n",
            "*\" restore \"*) echo boom >&2; exit 1 ;;\n",
            "*) exec git \"$@\" ;;\nesac\n"
        );
        fs::write(&script, text).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let repo = two_changes();
        let data = TempDir::new().unwrap();

        let saved = shelve_with(&repo, &runner_with(Some(script)), data.path(), || {});

        let error = branches::text_of(saved.unwrap_err());
        assert!(error.contains("boom"), "{error}");
        assert!(
            error.contains("but not all of them were rolled back"),
            "{error}"
        );
        assert!(error.contains("Before Update [Changes]"), "{error}");
        assert_eq!(read(&repo, "a.txt"), "local a\n");
        assert_eq!(alive(data.path()).len(), 1);
    }

    #[test]
    fn a_failed_store_rolls_nothing_back() {
        let repo = two_changes();
        let data = TempDir::new().unwrap();
        fs::write(data.path().join("shelf"), "not a folder").unwrap();

        let saved = shelve_with(&repo, &runner(), data.path(), || {});

        assert!(saved.is_err());
        assert_eq!(read(&repo, "a.txt"), "local a\n");
        assert_eq!(read(&repo, "b.txt"), "local b\n");
        assert!(snapshots::list(data.path())
            .iter()
            .all(|copy| copy.reason != SHELVE_REASON));
    }

    #[test]
    fn the_progress_says_shelving_and_unshelving() {
        let repo = with_change();
        let data = TempDir::new().unwrap();
        let runner = runner();
        let texts = Arc::new(Mutex::new(Vec::new()));
        let seen = texts.clone();
        runner.listen(move |event| seen.lock().unwrap().push(event.text.clone()));

        let saved = shelve_local(&repo, &runner, data.path()).unwrap();
        restore_recorded(&repo, &runner, data.path(), saved);

        let texts = texts.lock().unwrap();
        assert!(
            texts.iter().any(|text| text == "Shelving changes…"),
            "{texts:?}"
        );
        assert!(
            texts.iter().any(|text| text == "Unshelving changes…"),
            "{texts:?}"
        );
    }

    #[test]
    fn the_stash_policy_keeps_the_stash() {
        let repo = with_change();
        let data = TempDir::new().unwrap();
        settings::save_clean_tree(data.path(), CleanTree::Stash).unwrap();

        let saved = shelve_local(&repo, &runner(), data.path()).unwrap();

        assert_eq!(saved.kind, CleanTree::Stash);
        assert_eq!(repo.git(&["rev-parse", "stash@{0}"]).trim(), saved.sha);
        assert_eq!(shelf::list(data.path()), []);
        // This build's stash leaves no record for `Restore Changes` (decision 10).
        assert_eq!(pending(repo.path(), data.path()), Ok(Vec::new()));
        assert_eq!(saver(data.path()), CleanTree::Stash);
    }
}
