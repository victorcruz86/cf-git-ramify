// File history without an index, as 241 computes it when the log has no index: `GitLogHistoryHandler`
// runs `GitFileHistory` with `fullHistory = true` (`GitLogHistoryHandler.kt:84-91`,
// `GitFileHistory.kt:63-107`); the `File Revisions` popup runs it with `fullHistory = false`
// (`GitHistoryProvider`, `GitFileHistory.kt:193-200`).

use std::collections::{HashSet, VecDeque};

use serde::Serialize;

use crate::git::{self, GitError};
use crate::log::details::{self, Block};

// `VcsConfiguration.MAXIMUM_HISTORY_ROWS` with `LIMIT_HISTORY` on (`GitHistoryProvider.getHistoryLimitArgs`):
// each walk stops there, and 241 says nothing about it in the new history.
pub const MAX_COUNT: u32 = 1000;
// `GitLogUtil.LOG_ALL` after `HEAD`: what an empty Branch filter starts from.
const ALL_REF_FLAGS: [&str; 3] = ["--branches", "--remotes", "--tags"];
const HEAD: &str = "HEAD";
const FORMAT: &str = "--format=%x01%H%x00%P";
// As log/store.rs: with `log.showSignature` in the user's configuration, `log` and `show` print the
// signature check (`No signature`, gpg's lines) before each record, mixed with the records.
pub const NO_SIGNATURES: [&str; 2] = ["-c", "log.showSignature=false"];

// A commit of the history, with the path the file has in it (`GitFileRevision.getPath`: the second
// path of a rename, else the first) and the git letter of its first status (`A`, `M`, `D`, `R`…).
// A merge whose diff git does not print has no status, and the path its walk started from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRow {
    pub hash: String,
    pub path: String,
    pub status: String,
}

impl HistoryRow {
    pub fn deleted(&self) -> bool {
        self.status == "D"
    }
}

// What git runs: the task's, which a cancellation stops.
pub type Git<'a> = dyn FnMut(&[&str]) -> Result<String, GitError> + 'a;

// Where a walk starts: every reference (`GitLogUtil.LOG_ALL`, with `HEAD` when there is one), or
// revisions, which only ever reach git after `--end-of-options`: a branch called `--all` is that
// branch, never the option.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Starts {
    AllRefs { head: bool },
    Revisions(Vec<String>),
}

// `FileHistoryStart`: where a walk starts and the path it follows.
struct Start {
    revisions: Starts,
    path: String,
}

// `GitLogProvider.getBranchLikeFilterParameters`: a revision for `History Up to Here`, which has no
// Branch filter (`GitLogHistoryHandler.getSupportedFilters`); else the branches the filter matches,
// or every reference when it is empty. `matching` are the names of the branches that match.
pub fn starting_revisions(
    revision: Option<&str>,
    branches: &[String],
    matching: Vec<String>,
) -> Starts {
    match revision {
        Some(revision) => Starts::Revisions(vec![revision.to_string()]),
        None if branches.is_empty() => Starts::AllRefs { head: true },
        None => Starts::Revisions(matching),
    }
}

// What a walk takes: 241's file history (`fullHistory`), the `File Revisions` popup (each walk
// stops at its first addition, and a merge parent without a record is not asked about), or a
// folder, whose additions are not renames (`!path.isDirectory`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Walk {
    Full,
    FirstAddition,
    Folder,
}

// `GitFileHistory.load`. Every addition is a candidate for a rename (`fullHistory`; without it, only
// the first of each walk), and each rename starts a new walk from the parent with the old path. A
// commit is listed once (`visitedCommits`).
pub fn file_history(
    git: &mut Git,
    path: &str,
    starts: &Starts,
    mode: Walk,
) -> Result<Vec<HistoryRow>, GitError> {
    let mut visited = HashSet::new();
    let mut rows = Vec::new();
    let mut queue = VecDeque::from([Start {
        revisions: starts.clone(),
        path: path.to_string(),
    }]);
    while let Some(start) = queue.pop_front() {
        let additions = walk(git, &start, mode, &mut visited, &mut rows)?;
        // As 241's `if (lastCommits.isEmpty()) return`: a walk without additions ends the whole
        // history, even with other starts still waiting.
        if additions.is_empty() {
            break;
        }
        for commit in additions {
            queue.extend(renames_before(git, &commit, &start.path, mode)?);
        }
    }
    Ok(rows)
}

// `GitFileHistory.createLogHandler` with `git.file.history.full`. Only rama's own flags of
// `LOG_ALL` go before `--end-of-options`; every name and hash goes after it.
fn log_args<'a>(start: &'a Start, max_count: &'a str) -> Vec<&'a str> {
    let mut args = git::exact(&[
        NO_SIGNATURES[0],
        NO_SIGNATURES[1],
        "--literal-pathspecs",
        "log",
        "--name-status",
        "-z",
        FORMAT,
        "--encoding=UTF-8",
        "--full-history",
        "--simplify-merges",
        max_count,
    ]);
    match &start.revisions {
        Starts::AllRefs { head } => {
            if *head {
                args.push(HEAD);
            }
            args.extend(ALL_REF_FLAGS);
            args.push("--end-of-options");
        }
        Starts::Revisions(names) => {
            args.push("--end-of-options");
            args.extend(names.iter().map(String::as_str));
        }
    }
    args.extend(["--", start.path.as_str()]);
    args
}

// `GitFileHistory.runGitLog`: the commits of one walk, and those where the file was added. Without
// `fullHistory` the walk ends at its first addition (`skipFurtherOutput`).
fn walk(
    git: &mut Git,
    start: &Start,
    mode: Walk,
    visited: &mut HashSet<String>,
    rows: &mut Vec<HistoryRow>,
) -> Result<Vec<String>, GitError> {
    let max_count = format!("--max-count={MAX_COUNT}");
    let output = git(&log_args(start, &max_count))?;
    let mut additions = Vec::new();
    for Block { hash, files, .. } in details::read_blocks(&output) {
        if mode == Walk::FirstAddition && !additions.is_empty() {
            break;
        }
        if !visited.insert(hash.clone()) {
            continue;
        }
        let first = files.first();
        let status = first.map_or(String::new(), |file| file.status[..1].to_string());
        if status == "A" && mode != Walk::Folder {
            additions.push(hash.clone());
        }
        rows.push(HistoryRow {
            hash,
            path: first.map_or_else(|| start.path.clone(), |file| file.path.clone()),
            status,
        });
    }
    Ok(additions)
}

// `VcsFileStatusInfo.isRenamed`: a rename or copy between two different paths. The old one.
fn renamed_from(block: &Block) -> Option<String> {
    block.files.iter().find_map(|file| {
        let renamed = file.status.starts_with(['R', 'C']);
        file.old_path
            .as_ref()
            .filter(|old| renamed && **old != file.path)
            .cloned()
    })
}

// `GitFileHistory.getParentsAndPathsIfRename`: `git show -M --diff-merges=separate --follow` of the
// addition (`GitCommitRequirements` with `DIFF_TO_PARENTS`). As in 241, the records are paired with
// the parents in order; the parents left without one ask `git diff` (`GitLogHistoryHandler.getRename`).
fn renames_before(
    git: &mut Git,
    commit: &str,
    path: &str,
    mode: Walk,
) -> Result<Vec<Start>, GitError> {
    let args = git::exact(&[
        NO_SIGNATURES[0],
        NO_SIGNATURES[1],
        "--literal-pathspecs",
        "show",
        "-M",
        "--diff-merges=separate",
        "--follow",
        "--name-status",
        "-z",
        FORMAT,
        "--encoding=UTF-8",
        "--end-of-options",
        commit,
        "--",
        path,
    ]);
    let records = details::read_blocks(&git(&args)?);
    let Some(parents) = records.first().map(|record| record.parents.clone()) else {
        return Ok(Vec::new());
    };
    let mut renames: Vec<(String, String)> = parents
        .iter()
        .zip(&records)
        .filter_map(|(parent, record)| Some((parent.clone(), renamed_from(record)?)))
        .collect();
    let asked = if mode == Walk::Full { parents.len() } else { 0 };
    for parent in parents.iter().take(asked).skip(records.len()) {
        if let Some(old) = rename_between(git, parent, commit, path)? {
            renames.push((parent.clone(), old));
        }
    }
    Ok(renames
        .into_iter()
        .map(|(parent, old)| Start {
            revisions: Starts::Revisions(vec![parent]),
            path: old,
        })
        .collect())
}

// `GitLogHistoryHandler.getRename`: the only rename to `path` between the two commits, by its old
// path; none when git reports none or several (`RenamesCollector.getSingleRename`).
fn rename_between(
    git: &mut Git,
    before: &str,
    after: &str,
    path: &str,
) -> Result<Option<String>, GitError> {
    let range = format!("{before}..{after}");
    let args = git::exact(&[
        "--literal-pathspecs",
        "diff",
        "-M",
        "--diff-filter=R",
        "--name-status",
        "-z",
        "--encoding=UTF-8",
        "--follow",
        "--end-of-options",
        &range,
        "--",
        path,
    ]);
    let output = git(&args)?;
    let mut tokens = output.split('\0').filter(|token| !token.is_empty());
    let mut renames = Vec::new();
    while let Some(status) = tokens.next() {
        let (Some(old), Some(new)) = (tokens.next(), tokens.next()) else {
            break;
        };
        if status.starts_with('R') && old != new {
            renames.push(old.to_string());
        }
    }
    Ok(if renames.len() == 1 {
        renames.pop()
    } else {
        None
    })
}

// `VcsUtil.getLastCommitPath`: a file renamed in the index and not committed yet has its history at
// the old path. Without a HEAD there is nothing to compare, and the path stays.
pub fn last_commit_path(git: &mut Git, path: &str) -> String {
    let args = git::exact(&[
        "diff",
        "--cached",
        "-M",
        "--diff-filter=R",
        "--name-status",
        "-z",
    ]);
    let Ok(output) = git(&args) else {
        return path.to_string();
    };
    let tokens: Vec<&str> = output.split('\0').collect();
    tokens
        .chunks(3)
        .find(|entry| entry.len() == 3 && entry[0].starts_with('R') && entry[2] == path)
        .map_or_else(|| path.to_string(), |entry| entry[1].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempRepo;
    use std::path::Path;
    use std::process::Stdio;

    fn real(repo: &Path) -> impl FnMut(&[&str]) -> Result<String, GitError> + '_ {
        move |args| git::run(repo, args).map(|out| String::from_utf8_lossy(&out).into_owned())
    }

    fn revisions(starts: &[&str]) -> Starts {
        Starts::Revisions(starts.iter().map(|start| start.to_string()).collect())
    }

    fn history(repo: &TempRepo, path: &str, starts: &[&str]) -> Vec<(String, String, String)> {
        walked(repo, path, &revisions(starts), Walk::Full)
    }

    fn walked(
        repo: &TempRepo,
        path: &str,
        starts: &Starts,
        mode: Walk,
    ) -> Vec<(String, String, String)> {
        file_history(&mut real(repo.path()), path, starts, mode)
            .unwrap()
            .into_iter()
            .map(|row| (row.hash, row.path, row.status))
            .collect()
    }

    fn head(repo: &TempRepo, revision: &str) -> String {
        repo.git(&["rev-parse", revision]).trim().to_string()
    }

    fn row(hash: &str, path: &str, status: &str) -> (String, String, String) {
        (hash.to_string(), path.to_string(), status.to_string())
    }

    // Enough lines for git to see a rename after an edit.
    fn text(extra: &str) -> Vec<u8> {
        let mut text: String = (1..=20).map(|line| format!("line {line}\n")).collect();
        text.push_str(extra);
        text.into_bytes()
    }

    #[test]
    fn history_follows_a_rename_from_the_parent_with_the_old_path() {
        let repo = TempRepo::new();
        let added = repo.commit("old.txt", &text(""), "add");
        let edited = repo.commit("old.txt", &text("edit\n"), "edit");
        repo.git(&["mv", "old.txt", "new.txt"]);
        repo.git(&["commit", "-q", "-m", "rename"]);
        let renamed = head(&repo, "HEAD");
        let after = repo.commit("new.txt", &text("edit\nafter\n"), "after");

        assert_eq!(
            history(&repo, "new.txt", &["HEAD"]),
            vec![
                row(&after, "new.txt", "M"),
                row(&renamed, "new.txt", "A"),
                row(&edited, "old.txt", "M"),
                row(&added, "old.txt", "A"),
            ]
        );
    }

    // Review Focus 2. `main`: `old.txt` added and edited; `side` renames it to `new.txt` and edits it;
    // `main` gets an unrelated commit, merges `side`, deletes `new.txt` and adds it again.
    //
    // 241's algorithm, by hand:
    // - walk 1, `log HEAD -- new.txt`: the edit after the readd (M), readd (A), delete (D), side edit
    //   (M), side rename (A, since the old path is outside the pathspec). The merge is not there:
    //   `--simplify-merges` drops it, since its `main` parent has no history of `new.txt` and it is
    //   the same as `side`;
    // - the additions: `show --follow readd` finds no rename (nothing was deleted there);
    //   `show --follow rename` gives `R old.txt new.txt` against its parent `edit`;
    // - walk 2, `log edit -- old.txt`: edit (M), add (A); `show --follow add` has no parent;
    // - no more starts.
    #[test]
    fn a_rename_on_a_side_branch_and_a_delete_and_readd_give_241_rows() {
        let repo = TempRepo::new();
        let added = repo.commit("old.txt", &text(""), "add");
        let edited = repo.commit("old.txt", &text("edit\n"), "edit");
        repo.git(&["checkout", "-q", "-b", "side"]);
        repo.git(&["mv", "old.txt", "new.txt"]);
        repo.git(&["commit", "-q", "-m", "rename"]);
        let renamed = head(&repo, "HEAD");
        let side_edit = repo.commit("new.txt", &text("edit\nside\n"), "side edit");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("other.txt", b"other\n", "unrelated");
        repo.git(&["merge", "-q", "--no-ff", "-m", "merge side", "side"]);
        repo.git(&["rm", "-q", "new.txt"]);
        repo.git(&["commit", "-q", "-m", "delete"]);
        let deleted = head(&repo, "HEAD");
        let readded = repo.commit("new.txt", b"something else\n", "readd");
        let last = repo.commit("new.txt", b"something else\nmore\n", "after readd");

        assert_eq!(
            history(&repo, "new.txt", &["HEAD"]),
            vec![
                row(&last, "new.txt", "M"),
                row(&readded, "new.txt", "A"),
                row(&deleted, "new.txt", "D"),
                row(&side_edit, "new.txt", "M"),
                row(&renamed, "new.txt", "A"),
                row(&edited, "old.txt", "M"),
                row(&added, "old.txt", "A"),
            ]
        );
    }

    type Calls = std::rc::Rc<std::cell::RefCell<Vec<String>>>;

    // A script of git: each command gets the next answer, and the commands are kept.
    fn scripted(answers: Vec<&str>) -> (impl FnMut(&[&str]) -> Result<String, GitError>, Calls) {
        let calls = Calls::default();
        let seen = calls.clone();
        let mut answers: VecDeque<String> = answers.into_iter().map(String::from).collect();
        let git = move |args: &[&str]| {
            seen.borrow_mut().push(args.join(" "));
            Ok(answers.pop_front().unwrap_or_default())
        };
        (git, calls)
    }

    // Git never prints the diff of a merge in `log`, so only a script reaches the parents that `show`
    // leaves without a record: they go through `diff -M --diff-filter=R`, and the rename restarts there.
    #[test]
    fn merge_parents_without_a_record_ask_git_diff_for_the_rename() {
        let (merge, first, second) = ("m".repeat(40), "f".repeat(40), "s".repeat(40));
        let log = format!("\u{1}{merge}\0{first} {second}\0\nA\0new.txt\0");
        let show = format!("\u{1}{merge}\0{first} {second}\0\nA\0new.txt\0");
        let diff = "R090\0old.txt\0new.txt\0".to_string();
        let (mut git, calls) = scripted(vec![&log, &show, &diff, ""]);

        let rows = file_history(&mut git, "new.txt", &revisions(&["HEAD"]), Walk::Full).unwrap();

        assert_eq!(rows.len(), 1);
        let calls = calls.borrow();
        assert!(calls[2].contains("diff -M --diff-filter=R"), "{}", calls[2]);
        assert!(
            calls[2].ends_with(&format!("{second}..{merge} -- new.txt")),
            "{}",
            calls[2]
        );
        assert!(
            calls[3].contains(&format!("--end-of-options {second} -- old.txt")),
            "{}",
            calls[3]
        );
    }

    // `git fast-import` makes the commits at once: one `git commit` each would take too long.
    // `old.txt` is added and edited `before` times, renamed to `new.txt`, and edited `after` times.
    fn long_history(repo: &TempRepo, before: u32, after: u32) {
        let mut stream = String::new();
        let mut mark = 0;
        let mut commit = |stream: &mut String, change: String| {
            mark += 1;
            let message = format!("commit {mark}");
            *stream += &format!(
                "commit refs/heads/main\nmark :{mark}\nauthor A <a@a.com> {mark} +0000\ncommitter A <a@a.com> {mark} +0000\ndata {}\n{message}\n",
                message.len()
            );
            if mark > 1 {
                *stream += &format!("from :{}\n", mark - 1);
            }
            *stream += &change;
            *stream += "\n";
        };
        let content = |number: u32| format!("version {number}\n");
        for number in 1..=before {
            let data = content(number);
            commit(
                &mut stream,
                format!("M 100644 inline old.txt\ndata {}\n{data}\n", data.len()),
            );
        }
        commit(&mut stream, "R old.txt new.txt\n".to_string());
        for number in 1..=after {
            let data = content(before + number);
            commit(
                &mut stream,
                format!("M 100644 inline new.txt\ndata {}\n{data}\n", data.len()),
            );
        }
        let mut child = crate::test_support::git_command()
            .current_dir(repo.path())
            .args(["fast-import", "--quiet"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        std::io::Write::write_all(&mut child.stdin.take().unwrap(), stream.as_bytes()).unwrap();
        assert!(child.wait().unwrap().success());
        repo.git(&["reset", "-q", "--hard", "main"]);
    }

    // Review Focus 2: each walk takes at most 1,000 commits, and nothing says the rest is missing.
    #[test]
    fn more_than_1000_commits_stop_at_1000_per_walk_without_a_notification() {
        let repo = TempRepo::new();
        long_history(&repo, 1100, 100);

        let rows = history(&repo, "new.txt", &["HEAD"]);

        // 100 edits and the rename on the new path, then 1,000 of the 1,100 commits of the old one.
        assert_eq!(rows.len(), 101 + 1000);
        assert!(rows[..101].iter().all(|(_, path, _)| path == "new.txt"));
        assert_eq!(rows[100].2, "A");
        assert!(rows[101..]
            .iter()
            .all(|(_, path, status)| path == "old.txt" && status == "M"));
        assert_eq!(rows.last().unwrap().0, head(&repo, "main~1100"));
    }

    #[test]
    fn a_walk_over_1000_without_an_addition_ends_there() {
        let repo = TempRepo::new();
        long_history(&repo, 1, 1200);

        assert_eq!(history(&repo, "new.txt", &["HEAD"]).len(), 1000);
    }

    #[test]
    fn the_branch_filter_defaults_to_head_and_empty_means_all_refs() {
        let repo = TempRepo::new();
        let base = repo.commit("a.txt", b"1\n", "base");
        repo.git(&["checkout", "-q", "-b", "other"]);
        let other = repo.commit("a.txt", b"2\n", "on other");
        repo.git(&["checkout", "-q", "main"]);
        let main = repo.commit("a.txt", b"3\n", "on main");
        let hashes = |starts: Starts| -> Vec<String> {
            file_history(&mut real(repo.path()), "a.txt", &starts, Walk::Full)
                .unwrap()
                .into_iter()
                .map(|row| row.hash)
                .collect()
        };

        let head_only = starting_revisions(None, &["HEAD".to_string()], vec!["HEAD".to_string()]);
        let all = starting_revisions(None, &[], Vec::new());

        assert_eq!(head_only, revisions(&["HEAD"]));
        assert_eq!(all, Starts::AllRefs { head: true });
        assert_eq!(hashes(head_only), vec![main.clone(), base.clone()]);
        let mut every = hashes(all);
        every.sort();
        let mut expected = vec![main, other, base];
        expected.sort();
        assert_eq!(every, expected);
    }

    #[test]
    fn a_revision_starts_the_history_whatever_the_branch_filter_says() {
        let repo = TempRepo::new();
        let base = repo.commit("a.txt", b"1\n", "base");
        let middle = repo.commit("a.txt", b"2\n", "middle");
        repo.commit("a.txt", b"3\n", "last");

        let starts = starting_revisions(Some(&middle), &["main".to_string()], vec!["main".into()]);

        assert_eq!(starts, revisions(&[&middle]));
        let rows = file_history(&mut real(repo.path()), "a.txt", &starts, Walk::Full).unwrap();
        let hashes: Vec<&str> = rows.iter().map(|row| row.hash.as_str()).collect();
        assert_eq!(hashes, vec![middle.as_str(), base.as_str()]);
    }

    // Through the index with the exact bytes: APFS keeps one name on disk for the NFD and NFC forms.
    fn commit_exact(repo: &TempRepo, name: &str, contents: &[u8], message: &str) -> String {
        let scratch = repo.path().join(".scratch");
        std::fs::write(&scratch, contents).unwrap();
        let blob = repo.git(&["hash-object", "-w", ".scratch"]);
        std::fs::remove_file(scratch).unwrap();
        index_add(repo, blob.trim(), name);
        repo.git(&["commit", "-q", "-m", message]);
        head(repo, "HEAD")
    }

    fn index_add(repo: &TempRepo, blob: &str, name: &str) {
        let info = format!("100644,{blob},{name}");
        let exact = ["-c", "core.precomposeunicode=false", "update-index"];
        repo.git(&[&exact[..], &["--add", "--cacheinfo", &info]].concat());
    }

    fn rename_exact(repo: &TempRepo, old: &str, new: &str, message: &str) -> String {
        let blob = repo.git(&[
            "-c",
            "core.precomposeunicode=false",
            "rev-parse",
            &format!("HEAD:{old}"),
        ]);
        let exact = ["-c", "core.precomposeunicode=false", "update-index"];
        repo.git(&[&exact[..], &["--force-remove", "--", old]].concat());
        index_add(repo, blob.trim(), new);
        repo.git(&["commit", "-q", "-m", message]);
        head(repo, "HEAD")
    }

    // Review Focus 3: a name git could read as a pattern, an option or another normalization reaches
    // only its own file, also across a rename.
    #[test]
    fn a_path_with_glob_characters_has_its_own_history() {
        let repo = TempRepo::new();
        repo.git(&["config", "core.precomposeunicode", "true"]);
        let (nfd, nfc) = ("cafe\u{301}.txt", "caf\u{e9}.txt");
        let star = commit_exact(&repo, "a*.txt", &text("star\n"), "star");
        commit_exact(&repo, "ab.txt", b"plain\n", "plain");
        let dash = commit_exact(&repo, "-x [1].txt", b"dash\n", "dash");
        commit_exact(&repo, "-x 1.txt", b"no brackets\n", "no brackets");
        let magic = commit_exact(&repo, ":(glob)b.txt", b"magic\n", "magic");
        commit_exact(&repo, "b.txt", b"b\n", "b");
        let decomposed = commit_exact(&repo, nfd, b"nfd\n", "nfd");
        let composed = commit_exact(&repo, nfc, b"nfc\n", "nfc");
        let moved = rename_exact(&repo, "a*.txt", "a?.txt", "rename star");

        assert_eq!(
            history(&repo, "a?.txt", &["HEAD"]),
            vec![row(&moved, "a?.txt", "A"), row(&star, "a*.txt", "A")]
        );
        assert_eq!(
            history(&repo, "-x [1].txt", &["HEAD"]),
            vec![row(&dash, "-x [1].txt", "A")]
        );
        assert_eq!(
            history(&repo, ":(glob)b.txt", &["HEAD"]),
            vec![row(&magic, ":(glob)b.txt", "A")]
        );
        assert_eq!(
            history(&repo, nfd, &["HEAD"]),
            vec![row(&decomposed, nfd, "A")]
        );
        assert_eq!(
            history(&repo, nfc, &["HEAD"]),
            vec![row(&composed, nfc, "A")]
        );
    }

    // `GitFileHistory` with `fullHistory = false`, as the `File Revisions` popup reads it: each walk
    // ends at its first addition, which still follows a rename.
    #[test]
    fn the_file_revisions_walk_stops_at_the_first_addition_of_each_walk() {
        let repo = TempRepo::new();
        let added = repo.commit("old.txt", &text(""), "add");
        repo.git(&["mv", "old.txt", "a.txt"]);
        repo.git(&["commit", "-q", "-m", "rename"]);
        let renamed = head(&repo, "HEAD");
        repo.git(&["rm", "-q", "a.txt"]);
        repo.git(&["commit", "-q", "-m", "delete"]);
        let readded = repo.commit("a.txt", b"other\n", "readd");
        let last = repo.commit("a.txt", b"other\nmore\n", "edit");

        let first = walked(&repo, "a.txt", &revisions(&["HEAD"]), Walk::FirstAddition);
        let full = history(&repo, "a.txt", &["HEAD"]);

        assert_eq!(
            first,
            vec![row(&last, "a.txt", "M"), row(&readded, "a.txt", "A")]
        );
        assert_eq!(full.len(), 5);
        assert_eq!(full[3], row(&renamed, "a.txt", "A"));
        assert_eq!(full[4], row(&added, "old.txt", "A"));
        // A rename found at the first addition still starts the next walk.
        repo.commit("b.txt", &text(""), "add b");
        let b_added = head(&repo, "HEAD");
        repo.git(&["mv", "b.txt", "c.txt"]);
        repo.git(&["commit", "-q", "-m", "rename b"]);
        let c_renamed = head(&repo, "HEAD");
        assert_eq!(
            walked(&repo, "c.txt", &revisions(&["HEAD"]), Walk::FirstAddition),
            vec![row(&c_renamed, "c.txt", "A"), row(&b_added, "b.txt", "A")]
        );
    }

    #[test]
    fn without_full_history_a_merge_parent_without_a_record_is_not_asked() {
        let (merge, first, second) = ("m".repeat(40), "f".repeat(40), "s".repeat(40));
        let log = format!("\u{1}{merge}\0{first} {second}\0\nA\0new.txt\0");
        let show = format!("\u{1}{merge}\0{first} {second}\0\nA\0new.txt\0");
        let (mut git, calls) = scripted(vec![&log, &show]);

        file_history(
            &mut git,
            "new.txt",
            &revisions(&["HEAD"]),
            Walk::FirstAddition,
        )
        .unwrap();

        assert_eq!(calls.borrow().len(), 2);
    }

    // A folder's additions are files of it, not renames of it (`!path.isDirectory`): one walk.
    #[test]
    fn a_folder_walk_has_no_rename_candidates() {
        let repo = TempRepo::new();
        let one = repo.commit("dir/a.txt", b"1\n", "one");
        let two = repo.commit("dir/b.txt", b"2\n", "two");
        let (mut git, calls) = {
            let calls = Calls::default();
            let seen = calls.clone();
            let path = repo.path().to_path_buf();
            let git = move |args: &[&str]| {
                seen.borrow_mut().push(args.join(" "));
                git::run(&path, args).map(|out| String::from_utf8_lossy(&out).into_owned())
            };
            (git, calls)
        };

        let rows = file_history(&mut git, "dir", &revisions(&["HEAD"]), Walk::Folder).unwrap();

        let hashes: Vec<&str> = rows.iter().map(|row| row.hash.as_str()).collect();
        assert_eq!(hashes, vec![two.as_str(), one.as_str()]);
        assert_eq!(calls.borrow().len(), 1);
    }

    // The project rule: nothing the user picks reaches git as an option. A branch can be called
    // `--all` or `-n1` (`update-ref`, a fetch refspec); it is that branch.
    #[test]
    fn a_ref_named_like_an_option_is_a_starting_revision_not_an_option() {
        let repo = TempRepo::new();
        let base = repo.commit("a.txt", b"1\n", "base");
        let older = repo.commit("a.txt", b"2\n", "older");
        repo.commit("a.txt", b"3\n", "newest");
        repo.git(&["update-ref", "refs/heads/--all", &base]);
        repo.git(&["update-ref", "refs/heads/-n1", &older]);

        let hashes = |name: &str| -> Vec<String> {
            history(&repo, "a.txt", &[name])
                .into_iter()
                .map(|(hash, _, _)| hash)
                .collect()
        };

        assert_eq!(hashes("--all"), vec![base.clone()]);
        assert_eq!(hashes("-n1"), vec![older, base]);
        let start = Start {
            revisions: revisions(&["--all"]),
            path: "a.txt".to_string(),
        };
        let args = log_args(&start, "--max-count=1000");
        let end = args
            .iter()
            .position(|arg| *arg == "--end-of-options")
            .unwrap();
        assert!(args.iter().position(|arg| *arg == "--all").unwrap() > end);
        let all = Start {
            revisions: Starts::AllRefs { head: true },
            path: "a.txt".to_string(),
        };
        let args = log_args(&all, "--max-count=1000");
        let end = args
            .iter()
            .position(|arg| *arg == "--end-of-options")
            .unwrap();
        assert_eq!(
            &args[end - 4..end],
            &["HEAD", "--branches", "--remotes", "--tags"]
        );
    }

    // With `log.showSignature` on, `log` and `show` print the signature check before each record:
    // the rows and the rename stay as without it.
    #[test]
    fn signatures_do_not_reach_the_history() {
        let repo = TempRepo::new();
        repo.sign_commits();
        let first = repo.commit("a.txt", b"one\n", "first");
        repo.git(&["mv", "a.txt", "b.txt"]);
        repo.git(&["commit", "-q", "-m", "rename"]);
        let renamed = head(&repo, "HEAD");
        let changed = repo.commit("b.txt", b"two\n", "change");
        assert!(repo
            .git(&["log", "-1", "--format=%s"])
            .contains("No signature"));

        for mode in [Walk::Full, Walk::FirstAddition] {
            assert_eq!(
                walked(&repo, "b.txt", &revisions(&["HEAD"]), mode),
                [
                    (changed.clone(), "b.txt".to_string(), "M".to_string()),
                    (renamed.clone(), "b.txt".to_string(), "A".to_string()),
                    (first.clone(), "a.txt".to_string(), "A".to_string()),
                ]
            );
        }
    }

    #[test]
    fn a_rename_in_the_index_has_its_history_at_the_old_path() {
        let repo = TempRepo::new();
        repo.commit("old.txt", &text(""), "add");
        repo.git(&["mv", "old.txt", "new.txt"]);

        assert_eq!(
            last_commit_path(&mut real(repo.path()), "new.txt"),
            "old.txt"
        );
        assert_eq!(
            last_commit_path(&mut real(repo.path()), "other.txt"),
            "other.txt"
        );
    }
}
