// The submodule update after Update Project (decision 7). 241's `GitSubmoduleUpdater` runs for a
// submodule in detached HEAD that is a registered root of its own, which never happens with one
// repository per window, so rama decides from the open repository, with `git submodule status
// --recursive` read before and after the repository's own update.
//
// No lost work, unlike 241: a submodule is only moved when, before the update, its detached HEAD
// was the commit the repository recorded, and it is still there. A submodule the user moved keeps
// its HEAD, so nothing ends up reachable only from its reflog; the cost is updating it by hand.
// On top of that, the invariant: an update never moves a HEAD that nothing else keeps (`is_kept`),
// in the submodule it names or in any submodule inside it. The cost: a submodule whose HEAD only
// the reflog holds is never updated by rama.
use std::collections::HashMap;
use std::path::{Component, Path};

use crate::git::GitError;
use crate::task::TaskContext;

const GITMODULES: &str = ".gitmodules";
const STATUS: [&str; 3] = ["submodule", "status", "--recursive"];
// `GitSubmoduleUpdater.kt:27-48`, with no `--init` and no `--remote`, limited to the submodules
// that qualify. git refuses to overwrite a submodule's local changes, and there is no `--force`.
const UPDATE: [&str; 6] = [
    "--literal-pathspecs",
    "submodule",
    "update",
    "--no-fetch",
    "--recursive",
    "--",
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    // `true`: `+`, its HEAD is not the commit the parent records. `false`: ` `, it is.
    moved: bool,
    // What is checked out in it.
    head: String,
    // Relative to the parent's work tree.
    path: String,
}

// What `git submodule status --recursive` showed before the update, with whether each submodule
// at its recorded commit had a detached HEAD.
pub struct Before {
    entries: Vec<Entry>,
    detached: Vec<String>,
    // For each submodule, the commit its parent's HEAD recorded for it: the repository's own
    // HEAD for a top-level one, and the checked-out commit of the submodule around a nested one.
    recorded: HashMap<String, String>,
}

fn moved_text(path: &str) -> String {
    format!(
        "Submodule {path} was not updated: its HEAD is not at the commit the repository records."
    )
}

// A line of `git submodule status`: the state (` ` checked out at the recorded commit, `+` at
// another one, `-` not initialized, `U` with conflicts), the commit checked out (the recorded one
// for `-`), the path relative to the parent and, for the initialized ones, `(<describe>)` when git
// found a name. The path is not quoted. Only the initialized ones without conflicts are kept.
fn parsed(root: &Path, line: &str) -> Option<Entry> {
    let (moved, rest) = match line.split_at_checked(1)? {
        (" ", rest) => (false, rest),
        ("+", rest) => (true, rest),
        _ => return None,
    };
    let (head, rest) = rest.split_once(' ')?;
    // A path can end in `(…)` itself: it is only the name when no folder has the whole text.
    let named = rest
        .strip_suffix(')')
        .and_then(|rest| rest.rsplit_once(" ("))
        .map(|(path, _name)| path);
    let path = match named {
        Some(path) if !root.join(rest).is_dir() => path,
        _ => rest,
    };
    Some(Entry {
        moved,
        head: head.to_string(),
        path: path.to_string(),
    })
}

// A path from git's output that goes to git after `--`: plain folders inside the work tree.
fn is_safe(root: &Path, path: &str) -> bool {
    let relative = Path::new(path);
    !path.is_empty()
        && !path.contains(['\n', '\0'])
        && relative
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        && root.join(relative).is_dir()
}

// `None`: there is nothing to decide from. Without `.gitmodules` there are no submodules to update
// (a gitlink without an entry there is not one), and a failing status is only logged: no update
// was attempted, so there is nothing to tell the user.
fn read(ctx: &TaskContext, repo: &Path) -> Option<Vec<Entry>> {
    if !repo.join(GITMODULES).is_file() {
        return None;
    }
    match ctx.git(&STATUS) {
        Ok(output) => Some(
            output
                .stdout
                .lines()
                .filter_map(|line| parsed(repo, line))
                .collect(),
        ),
        Err(GitError::Cancelled) => None,
        Err(error) => {
            eprintln!("git submodule status failed, the submodules are not updated: {error:?}");
            None
        }
    }
}

// `symbolic-ref -q HEAD` ends with 1, and nothing else, when HEAD is not a branch.
fn is_detached(ctx: &TaskContext, folder: &Path) -> bool {
    let folder = folder.to_string_lossy();
    matches!(
        ctx.git(&["-C", &folder, "symbolic-ref", "-q", "HEAD"]),
        Err(GitError::Failed { code: Some(1), .. })
    )
}

// Whether `path` lies inside the submodule at `parent`.
fn is_inside(path: &str, parent: &str) -> bool {
    path.strip_prefix(parent)
        .is_some_and(|rest| rest.starts_with('/'))
}

fn is_hash(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

// `<commit>:<path>` in the repository at `folder`, which is the gitlink when `path` is one.
fn gitlink_at(ctx: &TaskContext, folder: &Path, commit: &str, path: &str) -> Option<String> {
    let folder = folder.to_string_lossy();
    let spec = format!("{commit}:{path}");
    let output = ctx
        .git(&["-C", &folder, "rev-parse", "--verify", "-q", &spec])
        .ok()?;
    Some(output.stdout.trim().to_string()).filter(|hash| is_hash(hash))
}

// What the parents recorded, read before the update: each entry against the HEAD of its nearest
// parent, with the parents' states of that same read.
fn recorded_by_parents(
    ctx: &TaskContext,
    repo: &Path,
    entries: &[Entry],
) -> HashMap<String, String> {
    let head = ctx
        .git(&["rev-parse", "--verify", "-q", "HEAD"])
        .ok()
        .map(|output| output.stdout.trim().to_string())
        .filter(|hash| is_hash(hash));
    let mut recorded = HashMap::new();
    for entry in entries {
        let parent = entries
            .iter()
            .filter(|parent| is_inside(&entry.path, &parent.path))
            .max_by_key(|parent| parent.path.len());
        let found = match parent {
            Some(parent) if is_safe(repo, &parent.path) && is_hash(&parent.head) => {
                let inside = &entry.path[parent.path.len() + 1..];
                let folder = repo.join(&parent.path);
                gitlink_at(ctx, &folder, &parent.head, inside)
            }
            Some(_) => None,
            None => head
                .as_deref()
                .and_then(|head| gitlink_at(ctx, repo, head, &entry.path)),
        };
        if let Some(commit) = found {
            recorded.insert(entry.path.clone(), commit);
        }
    }
    recorded
}

// Before the repository's own update, under the same lock.
pub fn before(ctx: &TaskContext, repo: &Path) -> Option<Before> {
    let entries = read(ctx, repo)?;
    let detached = entries
        .iter()
        .filter(|entry| !entry.moved && is_detached(ctx, &repo.join(&entry.path)))
        .map(|entry| entry.path.clone())
        .collect();
    let recorded = recorded_by_parents(ctx, repo, &entries);
    Some(Before {
        entries,
        detached,
        recorded,
    })
}

// Something besides the reflog keeps the HEAD of the submodule `entry` shows: one of its own
// branches, tags or remote branches contains it, or it is the commit its parent's HEAD
// recorded before the update.
fn is_kept(ctx: &TaskContext, repo: &Path, before: &Before, entry: &Entry) -> bool {
    if before.recorded.get(&entry.path) == Some(&entry.head) {
        return true;
    }
    if !is_safe(repo, &entry.path) {
        return false;
    }
    let folder = repo.join(&entry.path).to_string_lossy().into_owned();
    // Not `refs/stash`: a stash on top of a detached commit holds it only until the next `pop`.
    let args = [
        "-C",
        &folder,
        "for-each-ref",
        "--contains",
        "HEAD",
        "--count=1",
        "refs/heads",
        "refs/tags",
        "refs/remotes",
    ];
    ctx.git(&args)
        .is_ok_and(|output| !output.stdout.trim().is_empty())
}

// What `decide` found after the update: the submodules to update, the nested ones inside them
// that `--recursive` reaches, and the Console lines about the ones left alone.
pub struct Decision {
    pub paths: Vec<String>,
    nested: Vec<String>,
    notes: Vec<String>,
}

// After it: the top-level submodules that were at their recorded commit with a detached HEAD, that
// still have that HEAD, detached, and whose recorded commit the update moved. One with a submodule
// inside that the user had moved is left alone too, as `--recursive` would move that one. Each
// submodule the user had moved, and that the update did not bring back, gets a line in the Console.
pub fn decide(ctx: &TaskContext, repo: &Path, before: &Before) -> Decision {
    let mut decision = Decision {
        paths: Vec::new(),
        nested: Vec::new(),
        notes: Vec::new(),
    };
    let Some(after) = read(ctx, repo) else {
        return decision;
    };
    // One that is gone after the update was not left alone.
    let still_moved = |path: &str| {
        let now = after.iter().find(|entry| entry.path == path);
        now.is_some_and(|entry| entry.moved)
    };
    let moved: Vec<&Entry> = before.entries.iter().filter(|entry| entry.moved).collect();
    for entry in moved.iter().filter(|entry| still_moved(&entry.path)) {
        decision.notes.push(moved_text(&entry.path));
    }
    let nested = |path: &str| {
        let mut parents = before.entries.iter();
        parents.any(|parent| is_inside(path, &parent.path))
    };
    let has_moved_inside = |path: &str| moved.iter().any(|entry| is_inside(&entry.path, path));
    let candidates = before
        .entries
        .iter()
        .filter(|entry| before.detached.contains(&entry.path))
        .filter(|entry| !nested(&entry.path) && !has_moved_inside(&entry.path))
        .filter(|entry| {
            after
                .iter()
                .any(|now| now.path == entry.path && now.moved && now.head == entry.head)
        })
        .filter(|entry| is_safe(repo, &entry.path))
        .filter(|entry| is_detached(ctx, &repo.join(&entry.path)));
    for entry in candidates {
        // Everything `--recursive` would touch: the submodule and every one inside it.
        let touched: Vec<&Entry> = after
            .iter()
            .filter(|now| now.path == entry.path || is_inside(&now.path, &entry.path))
            .collect();
        let unkept: Vec<&&Entry> = touched
            .iter()
            .filter(|now| !is_kept(ctx, repo, before, now))
            .collect();
        if unkept.is_empty() {
            decision.paths.push(entry.path.clone());
            let inside = touched.iter().filter(|now| now.path != entry.path);
            decision.nested.extend(inside.map(|now| now.path.clone()));
        } else {
            decision
                .notes
                .extend(unkept.iter().map(|now| moved_text(&now.path)));
        }
    }
    decision
}

// The Console lines of the last decision.
pub fn note(ctx: &TaskContext, decision: &Decision) {
    for text in &decision.notes {
        ctx.console_line(text);
    }
}

// The commit the repository's index records for `path` (stage 0, so a `:1:` in a path means nothing).
fn indexed_gitlink(ctx: &TaskContext, path: &str) -> Option<String> {
    let spec = format!(":0:{path}");
    let output = ctx.git(&["rev-parse", "--verify", "-q", &spec]).ok()?;
    Some(output.stdout.trim().to_string()).filter(|hash| is_hash(hash))
}

fn has_commit(ctx: &TaskContext, folder: &Path, commit: &str) -> bool {
    let folder = folder.to_string_lossy();
    let object = format!("{commit}^{{commit}}");
    ctx.git(&["-C", &folder, "cat-file", "-e", &object]).is_ok()
}

// As git's own submodule fetch runs: the URL came from a repository, not from the user, so a
// transport that `protocol.allow` leaves to the user (`file`, a local path) is refused unless the
// submodule's config allows it.
const FROM_REPOSITORY: [(&str, &str); 1] = [("GIT_PROTOCOL_FROM_USER", "0")];

// The remote lock of a submodule's fetch. A remote's name cannot hold a `:` (it must make a valid
// `refs/remotes/<name>/…`), so this is never one of the repository's own remotes.
fn remote_key(path: &str) -> String {
    format!("submodule:{path}")
}

// What `git submodule update` would fetch, fetched beforehand so that the check comes after it: in
// each submodule to update, and in each one inside it, whose new recorded commit is missing, a
// `git fetch` against its configured remote, through the Console and askpass. Not recursive: the
// nested ones are fetched here, each against what its parent's new commit records. Without the
// write lock: each fetch waits only for that submodule's remote lock.
pub fn fetch_missing(ctx: &TaskContext, repo: &Path, decision: &Decision) -> Result<(), GitError> {
    let mut needed: HashMap<&str, String> = HashMap::new();
    for path in &decision.paths {
        if let Some(commit) = indexed_gitlink(ctx, path) {
            needed.insert(path, commit);
        }
    }
    // Git lists a parent before what is inside it.
    let mut order: Vec<&String> = decision.paths.iter().chain(&decision.nested).collect();
    order.sort();
    for path in order {
        if !is_safe(repo, path) {
            continue;
        }
        if !needed.contains_key(path.as_str()) {
            let parent = needed
                .keys()
                .copied()
                .filter(|parent| is_inside(path, parent))
                .max_by_key(|parent| parent.len());
            let found = parent.and_then(|parent| {
                let inside = &path[parent.len() + 1..];
                gitlink_at(ctx, &repo.join(parent), &needed[parent], inside)
            });
            let Some(commit) = found else {
                continue;
            };
            needed.insert(path, commit);
        }
        let folder = repo.join(path);
        if !has_commit(ctx, &folder, &needed[path.as_str()]) {
            let folder = folder.to_string_lossy();
            let args = ["-C", &folder, "fetch", "--recurse-submodules=no"];
            ctx.on_remote(&remote_key(path), || {
                ctx.git_loud_with_env(&args, &FROM_REPOSITORY)
            })
            .ok_or(GitError::Cancelled)??;
        }
    }
    Ok(())
}

// In the task's repository, through the Console, without fetching: `fetch_missing` did that before
// the last check. The paths are literal. Under the write lock, right after that check.
pub fn update(ctx: &TaskContext, paths: &[String]) -> Result<(), GitError> {
    let mut args: Vec<&str> = UPDATE.to_vec();
    args.extend(paths.iter().map(String::as_str));
    ctx.git_loud(&args).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::RepoLocks;
    use crate::console::Console;
    use crate::task::Runner;
    use crate::test_support::{in_task, TempRepo};
    use std::fs;
    use std::sync::Arc;

    const HASH: &str = "fd09e3273b3ff7e28b07bbde6f4cdcf8b6633068";

    fn entry(moved: bool, path: &str) -> Entry {
        Entry {
            moved,
            head: HASH.to_string(),
            path: path.to_string(),
        }
    }

    fn in_repo<T: Send + 'static>(
        repo: &TempRepo,
        work: impl FnOnce(&TaskContext, &Path) -> T + Send + 'static,
    ) -> T {
        let runner = Runner::new(Arc::<RepoLocks>::default(), Arc::<Console>::default());
        let root = repo.path().to_path_buf();
        in_task(&runner, repo.path(), move |ctx| work(ctx, &root))
    }

    fn detached_before(repo: &TempRepo) -> Option<Vec<String>> {
        in_repo(repo, |ctx, root| {
            before(ctx, root).map(|before| before.detached)
        })
    }

    #[test]
    fn the_initialized_lines_give_their_state_head_and_path_without_the_name() {
        let root = Path::new("/nowhere");
        let line = |state: &str, rest: &str| format!("{state}{HASH} {rest}");
        let parse = |text: &str| parsed(root, text);

        assert_eq!(
            parse(&line(" ", "libs/lib (heads/main)")),
            Some(entry(false, "libs/lib"))
        );
        assert_eq!(
            parse(&line("+", "lib dir (remotes/origin/HEAD)")),
            Some(entry(true, "lib dir"))
        );
        assert_eq!(parse(&line(" ", "no name")), Some(entry(false, "no name")));
        assert_eq!(
            parse(&line(" ", "a (b) (v1.0-2-gabc)")),
            Some(entry(false, "a (b)"))
        );
        assert_eq!(parse(&line("-", "not/initialized")), None);
        assert_eq!(parse(&line("U", "conflicted")), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("é"), None);
    }

    #[test]
    fn a_folder_named_with_parentheses_keeps_them() {
        let repo = TempRepo::new();
        fs::create_dir(repo.path().join("odd (x)")).unwrap();
        let line = format!(" {HASH} odd (x)");

        assert_eq!(parsed(repo.path(), &line), Some(entry(false, "odd (x)")));
    }

    #[test]
    fn only_plain_folders_inside_the_work_tree_reach_git() {
        let repo = TempRepo::new();
        fs::create_dir_all(repo.path().join("libs/lib")).unwrap();
        fs::create_dir(repo.path().join("-dash")).unwrap();

        assert!(is_safe(repo.path(), "libs/lib"));
        assert!(is_safe(repo.path(), "-dash"));
        for path in [
            "",
            "missing",
            "/etc",
            "../x",
            "libs/../libs/lib",
            "./libs",
            "a\nb",
        ] {
            assert!(!is_safe(repo.path(), path), "{path:?}");
        }
    }

    #[test]
    fn the_detached_ones_at_their_recorded_commit_are_noted_before() {
        let parent = TempRepo::new();
        parent.commit("a.txt", b"a\n", "first");
        let _on_branch = parent.with_submodule("on branch");
        let loose = parent.with_submodule("libs/loose");
        let moved = parent.with_submodule("moved");
        let _gone = parent.with_submodule("gone");
        loose.git(&["checkout", "-q", "--detach"]);
        moved.commit("mine.txt", b"mine\n", "mine");
        moved.git(&["checkout", "-q", "--detach"]);
        parent.git(&["submodule", "deinit", "-q", "gone"]);

        assert_eq!(
            detached_before(&parent),
            Some(vec!["libs/loose".to_string()])
        );
    }

    #[test]
    fn a_nested_submodule_is_read_with_its_full_path() {
        let parent = TempRepo::new();
        parent.commit("a.txt", b"a\n", "first");
        let outer = parent.with_submodule("outer");
        let inner = outer.with_submodule("inner");
        inner.git(&["checkout", "-q", "--detach"]);
        // `outer` now records `inner`, and the parent records that commit of `outer`.
        parent.git(&["add", "outer"]);
        parent.git(&["commit", "-q", "-m", "record outer again"]);

        assert_eq!(
            detached_before(&parent),
            Some(vec!["outer/inner".to_string()])
        );
    }

    #[test]
    fn each_recorded_commit_is_read_from_its_nearest_parent_s_head() {
        let parent = TempRepo::new();
        parent.commit("a.txt", b"a\n", "first");
        let outer = parent.with_submodule("outer");
        let inner = outer.with_submodule("inner");
        parent.git(&["add", "outer"]);
        parent.git(&["commit", "-q", "-m", "record outer"]);
        let recorded_inner = inner.git(&["rev-parse", "HEAD"]).trim().to_string();
        let recorded_outer = outer.git(&["rev-parse", "HEAD"]).trim().to_string();
        // Staged only: the parent's HEAD still records the commit from before.
        let staged = outer.commit("y.txt", b"y\n", "staged in the parent only");
        parent.git(&["add", "outer"]);

        let recorded = in_repo(&parent, |ctx, root| before(ctx, root).unwrap().recorded);

        assert_eq!(recorded.get("outer"), Some(&recorded_outer));
        assert_ne!(recorded.get("outer"), Some(&staged));
        assert_eq!(recorded.get("outer/inner"), Some(&recorded_inner));
    }

    #[test]
    fn without_gitmodules_nothing_is_read() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"a\n", "first");

        assert_eq!(detached_before(&repo), None);
    }

    // A gitlink without an entry in `.gitmodules` makes `git submodule status` fail.
    #[test]
    fn a_failing_status_reads_nothing() {
        let parent = TempRepo::new();
        parent.commit("a.txt", b"a\n", "first");
        let _sub = parent.with_submodule("sub");
        parent.orphan_gitlink("orphan");

        assert_eq!(detached_before(&parent), None);
    }

    #[test]
    fn the_moved_text_is_rama_s_own() {
        assert_eq!(
            moved_text("libs/lib"),
            "Submodule libs/lib was not updated: its HEAD is not at the commit the repository records."
        );
    }
}
