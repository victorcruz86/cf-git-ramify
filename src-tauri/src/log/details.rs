use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::diff::{self, ChangedFile};
use crate::git::{self, GitError};
use crate::log::commands::LogRef;
use crate::log::filter::same_person;
use crate::log::refs::RefKind;
use crate::log::store::{CommitStore, Person};
use crate::log::view::Permanent;

// `CommitDetailsListPanel`: what is shown for a larger selection.
const MAX_COMMITS: usize = 50;
const FIELD_COUNT: usize = 9;
// `%B` is the only thing that can contain line breaks: the fields are separated by nulls.
// The limit is set by the caller: `vcs.log.max.changes.shown` belongs to the frontend.
const BLOCK_MARK: char = '\u{1}';
const BATCH_ARGS: [&str; 13] = [
    "-c",
    "log.showSignature=false",
    "-c",
    "log.showroot=true",
    "log",
    "--no-walk=unsorted",
    "--stdin",
    "--diff-merges=separate",
    "-M",
    "--name-status",
    "-z",
    "--format=%x01%H%x00%P",
    "--",
];
const FORMAT: &str =
    "--format=tformat:%H%x00%P%x00%an%x00%ae%x00%at%x00%cn%x00%ce%x00%ct%x00%B%x00";

// `links` carries each text of the message that names a commit of the log, with its full hash.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitDetails {
    pub hash: String,
    pub subject: String,
    pub body: String,
    pub author: String,
    pub author_email: String,
    pub author_time: i64,
    pub committer: String,
    pub committer_email: String,
    pub commit_time: i64,
    pub same_person: bool,
    pub refs: Vec<LogRef>,
    pub parents: Vec<String>,
    pub links: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeMode {
    Merged,
    Parents,
}

// `base` is what it is compared against. `parent` and `subject` only go in the "parents" mode of
// a merge, one group per parent.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeGroup {
    pub parent: Option<String>,
    pub subject: Option<String>,
    pub base: Option<String>,
    pub merge: bool,
    pub files: Vec<ChangedFile>,
    // `VcsLogUtil.getSize`: the changes against all its parents.
    pub size: u32,
}

#[derive(Debug, Serialize)]
pub struct SelectedCommit {
    pub hash: String,
    pub group: ChangeGroup,
}

// `commits` is empty if `max_size` exceeds the requested limit.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectedChanges {
    pub commits: Vec<SelectedCommit>,
    pub max_size: u32,
}

pub fn commit_details(
    repo: &Path,
    store: &CommitStore,
    hashes: &[String],
) -> Result<Vec<CommitDetails>, GitError> {
    let hashes = &hashes[..hashes.len().min(MAX_COMMITS)];
    if hashes.is_empty() {
        return Ok(Vec::new());
    }
    for hash in hashes {
        diff::check_hash(hash)?;
    }
    let mut args = vec![
        "-c",
        "log.showSignature=false",
        "log",
        "--no-walk=unsorted",
        "--encoding=UTF-8",
        FORMAT,
        "--end-of-options",
    ];
    args.extend(hashes.iter().map(String::as_str));
    args.push("--");
    let output = git::run(repo, &args)?;
    let commits = store.find(hashes.iter().map(String::as_str));
    let text = String::from_utf8_lossy(&output);
    let fields: Vec<&str> = text.split('\0').collect();
    Ok(fields
        .as_chunks::<FIELD_COUNT>()
        .0
        .iter()
        .map(|fields| read_details(fields, store, &commits))
        .collect())
}

fn read_details(
    fields: &[&str; FIELD_COUNT],
    store: &CommitStore,
    commits: &HashMap<&str, u32>,
) -> CommitDetails {
    let [hash, parents, author, author_email, author_time, committer, committer_email, commit_time, message] =
        fields;
    // The line break that `tformat` leaves after the previous commit.
    let hash = hash.trim_start();
    let person = |name: &str, email: &str| Person {
        name: name.into(),
        email: email.into(),
    };
    let (subject, body) = split_message(message.trim_end());
    CommitDetails {
        hash: hash.to_string(),
        same_person: same_person(
            &person(author, author_email),
            &person(committer, committer_email),
        ),
        links: resolved_links(store, message),
        subject: subject.to_string(),
        body: body.trim_start_matches('\n').to_string(),
        author: author.to_string(),
        author_email: author_email.to_string(),
        author_time: author_time.parse().unwrap_or(0),
        committer: committer.to_string(),
        committer_email: committer_email.to_string(),
        commit_time: commit_time.parse().unwrap_or(0),
        refs: commits.get(hash).map_or_else(Vec::new, |commit| {
            store
                .refs()
                .of_commit(*commit)
                .into_iter()
                .map(LogRef::from)
                .collect()
        }),
        parents: parents.split_whitespace().map(str::to_string).collect(),
    }
}

// `CommitPresentationUtil.getSubject`: up to the first blank line.
fn split_message(message: &str) -> (&str, &str) {
    match message.find("\n\n") {
        Some(separator) if separator > 0 => message.split_at(separator),
        _ => (message, ""),
    }
}

// `findHashes`: what looks like a hash is a link only if it belongs to a commit of the log.
fn resolved_links(store: &CommitStore, message: &str) -> BTreeMap<String, String> {
    static HASH: LazyLock<Regex> =
        LazyLock::new(|| Regex::new("(?i)[0-9a-f]{7,40}").expect("fixed pattern"));
    let mut links = BTreeMap::new();
    for found in HASH.find_iter(message) {
        let text = found.as_str();
        let prefix = text.to_ascii_lowercase();
        if let Some(hash) = store.with_prefix(&prefix) {
            links.insert(text.to_string(), hash.to_string());
        }
    }
    links
}

// `graph.getContainingBranches`: going up through the children from the commit.
pub fn branches_containing(store: &CommitStore, permanent: &Permanent, hash: &str) -> Vec<String> {
    let Some(commit) = store.find(std::iter::once(hash)).get(hash).copied() else {
        return Vec::new();
    };
    let mut seen = vec![false; store.len() as usize];
    seen[commit as usize] = true;
    let mut pending = vec![commit];
    while let Some(row) = pending.pop() {
        for edge in permanent.graph.ups.of(row) {
            if !std::mem::replace(&mut seen[edge.up as usize], true) {
                pending.push(edge.up);
            }
        }
    }
    let mut branches: Vec<_> = store
        .refs()
        .all
        .iter()
        .filter(|git_ref| git_ref.kind != RefKind::Tag && seen[git_ref.commit as usize])
        .collect();
    store.refs().sort_labels(&mut branches);
    branches
        .into_iter()
        .map(|git_ref| git_ref.name.to_string())
        .collect()
}

pub fn commit_changes(
    repo: &Path,
    store: &CommitStore,
    hash: &str,
    mode: ChangeMode,
) -> Result<Vec<ChangeGroup>, GitError> {
    let parents = diff::parents_of(repo, hash)?;
    let merge = parents.len() > 1;
    let mut changes = Vec::with_capacity(parents.len());
    for parent in &parents {
        changes.push(diff::changes_between(repo, Some(parent), hash)?);
    }
    let mut size: u32 = changes.iter().map(|files| files.len() as u32).sum();
    if !merge || mode == ChangeMode::Merged {
        let base = parents.first().cloned();
        let files = match (base.is_some(), merge) {
            (false, _) => diff::changes_between(repo, None, hash)?,
            (true, false) => changes.remove(0),
            (true, true) => merge_changes(changes),
        };
        if base.is_none() {
            size = files.len() as u32;
        }
        return Ok(vec![ChangeGroup {
            parent: None,
            subject: None,
            base,
            merge,
            files,
            size,
        }]);
    }
    let commits = store.find(parents.iter().map(String::as_str));
    Ok(parents
        .iter()
        .zip(changes)
        .map(|(parent, files)| ChangeGroup {
            parent: Some(parent.clone()),
            subject: commits
                .get(parent.as_str())
                .map(|commit| store.get(*commit).subject.to_string()),
            base: Some(parent.clone()),
            merge,
            files,
            size,
        })
        .collect())
}

// Like `GitDetailsCollector`: a single command for the whole selection, and `run` lets it be
// cancelled.
pub fn selected_changes(
    hashes: &[String],
    limit: Option<u32>,
    run: impl FnOnce(&[&str], &[u8]) -> Result<String, GitError>,
) -> Result<SelectedChanges, GitError> {
    for hash in hashes {
        diff::check_hash(hash)?;
    }
    let input: String = hashes.iter().map(|hash| format!("{hash}\n")).collect();
    let output = run(&BATCH_ARGS, input.as_bytes())?;
    let commits = read_batch(&output);
    let max_size = commits
        .iter()
        .map(|commit| commit.group.size)
        .max()
        .unwrap_or(0);
    let commits = match limit {
        Some(limit) if max_size > limit => Vec::new(),
        _ => commits,
    };
    Ok(SelectedChanges { commits, max_size })
}

pub(crate) struct Block {
    pub(crate) hash: String,
    pub(crate) parents: Vec<String>,
    pub(crate) files: Vec<ChangedFile>,
}

// After the format, `-z` leaves `\n` and the status-path pairs separated by nulls. With
// `--diff-merges=separate` each parent with differences repeats the commit header.
pub(crate) fn read_blocks(output: &str) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut tokens = output.split('\0');
    while let Some(token) = tokens.next() {
        let token = token.trim_start_matches('\n');
        if let Some(hash) = token.strip_prefix(BLOCK_MARK) {
            blocks.push(Block {
                hash: hash.to_string(),
                parents: tokens
                    .next()
                    .unwrap_or_default()
                    .split_whitespace()
                    .map(str::to_string)
                    .collect(),
                files: Vec::new(),
            });
        } else if let Some(block) = blocks.last_mut().filter(|_| !token.is_empty()) {
            let first = tokens.next().unwrap_or_default().to_string();
            let (path, old_path) = if token.starts_with(['R', 'C']) {
                (tokens.next().unwrap_or_default().to_string(), Some(first))
            } else {
                (first, None)
            };
            block.files.push(ChangedFile {
                path,
                old_path,
                status: token.to_string(),
            });
        }
    }
    blocks
}

fn read_batch(output: &str) -> Vec<SelectedCommit> {
    let blocks = read_blocks(output);
    blocks
        .chunk_by(|a, b| a.hash == b.hash)
        .map(|blocks| {
            let Block { hash, parents, .. } = &blocks[0];
            let merge = parents.len() > 1;
            let size = blocks.iter().map(|block| block.files.len() as u32).sum();
            // Git does not repeat the header of a parent without differences: then there is nothing
            // that changed against all of them.
            let files = if !merge {
                blocks[0].files.clone()
            } else if blocks.len() == parents.len() {
                let changes = blocks.iter().map(|block| block.files.clone()).collect();
                merge_changes(changes)
            } else {
                Vec::new()
            };
            SelectedCommit {
                hash: hash.clone(),
                group: ChangeGroup {
                    parent: None,
                    subject: None,
                    base: parents.first().cloned(),
                    merge,
                    files,
                    size,
                },
            }
        })
        .collect()
}

fn is_rename(file: &ChangedFile) -> bool {
    file.status.starts_with('R')
}

fn is_deletion(file: &ChangedFile) -> bool {
    file.status.starts_with('D')
}

// `VcsStatusMerger.merge`: only the paths that changed against all the parents count.
fn merge_changes(mut statuses: Vec<Vec<ChangedFile>>) -> Vec<ChangedFile> {
    let first = statuses.remove(0);
    let others: Vec<HashMap<String, ChangedFile>> = statuses
        .into_iter()
        .map(|files| {
            files
                .into_iter()
                .map(|file| (file.path.clone(), file))
                .collect()
        })
        .collect();
    first
        .into_iter()
        .filter_map(|file| {
            let rest: Option<Vec<&ChangedFile>> =
                others.iter().map(|files| files.get(&file.path)).collect();
            Some(merge_status(&file, &rest?))
        })
        .collect()
}

fn merge_status(first: &ChangedFile, rest: &[&ChangedFile]) -> ChangedFile {
    let kind = |file: &ChangedFile| file.status.as_bytes().first().copied();
    let plain = |status: &str| ChangedFile {
        path: first.path.clone(),
        old_path: None,
        status: status.to_string(),
    };
    if rest.iter().all(|file| kind(file) == kind(first)) {
        let same_origin = rest.iter().all(|file| file.old_path == first.old_path);
        if is_rename(first) && !same_origin {
            return plain("M");
        }
        return first.clone();
    }
    if is_deletion(first) || rest.iter().any(|file| is_deletion(file)) {
        plain("D")
    } else {
        plain("M")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::refs::RefKind;
    use crate::log::store;
    use crate::test_support::TempRepo;

    fn store_of(repo: &TempRepo) -> CommitStore {
        store::load(repo.path()).unwrap()
    }

    fn head(repo: &TempRepo) -> String {
        repo.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    fn one(repo: &TempRepo, hash: &str) -> CommitDetails {
        let store = store_of(repo);
        commit_details(repo.path(), &store, &[hash.to_string()])
            .unwrap()
            .remove(0)
    }

    fn paths(group: &ChangeGroup) -> Vec<&str> {
        group.files.iter().map(|file| &*file.path).collect()
    }

    fn containing(repo: &TempRepo, hash: &str) -> Vec<String> {
        let store = store_of(repo);
        let permanent = Permanent::from_store(&store, store.refs());
        branches_containing(&store, &permanent, hash)
    }

    // Base with `a.txt`; both branches change it and the merge resolves it. `b.txt` and `c.txt`
    // arrive without conflict, each from one side.
    fn merge_with_conflict() -> TempRepo {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"base\n", "base");
        repo.git(&["checkout", "-q", "-b", "other"]);
        repo.commit("a.txt", b"other\n", "on other");
        repo.commit("b.txt", b"b\n", "b on other");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("a.txt", b"main\n", "on main");
        repo.commit("c.txt", b"c\n", "c on main");
        let merge = crate::test_support::git_command()
            .args(["merge", "-q", "--no-ff", "-m", "merge", "other"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(!merge.status.success(), "a conflict was expected");
        std::fs::write(repo.path().join("a.txt"), "resolved\n").unwrap();
        repo.git(&["add", "-A"]);
        repo.git(&["commit", "-q", "--no-edit"]);
        repo
    }

    #[test]
    fn details_and_changes_come_out_the_same_without_the_log_loaded() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        let hash = repo.commit("b.txt", b"2\n", "two");
        let store = CommitStore::default();

        let details = commit_details(repo.path(), &store, std::slice::from_ref(&hash)).unwrap();
        let groups = commit_changes(repo.path(), &store, &hash, ChangeMode::Merged).unwrap();

        assert_eq!(details[0].subject, "two");
        assert_eq!(details[0].refs.len(), 0);
        assert_eq!(groups[0].files.len(), 1);
    }

    #[test]
    fn details_read_message_with_blank_lines() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "base");
        repo.git(&[
            "commit",
            "-q",
            "--allow-empty",
            "--cleanup=verbatim",
            "-m",
            "title\n\nfirst\n\n\nsecond\n",
        ]);

        let details = one(&repo, &head(&repo));

        assert_eq!(details.subject, "title");
        assert_eq!(details.body, "first\n\n\nsecond");
        assert_eq!(details.author, "Test");
        assert_eq!(details.author_email, "test@example.com");
        assert!(details.same_person);
    }

    #[test]
    fn details_with_several_unseparated_lines_has_no_separate_subject() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one\ntwo\n\nthree");

        let details = one(&repo, &head(&repo));

        assert_eq!(details.subject, "one\ntwo");
        assert_eq!(details.body, "three");
    }

    #[test]
    fn details_different_author_and_committer() {
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

        let details = one(&repo, &head(&repo));

        assert_eq!(details.author, "Test");
        assert_eq!(details.committer, "Another");
        assert_eq!(details.committer_email, "another@example.com");
        assert!(!details.same_person);
    }

    #[test]
    fn details_in_the_requested_order_with_parents_and_references() {
        let repo = TempRepo::new();
        let first = repo.commit("a.txt", b"1", "one");
        repo.git(&["tag", "v1"]);
        let second = repo.commit("a.txt", b"2", "two");
        let store = store_of(&repo);

        let details =
            commit_details(repo.path(), &store, &[first.clone(), second.clone()]).unwrap();

        assert_eq!(details[0].hash, first);
        assert_eq!(details[0].refs[0].kind, RefKind::Tag);
        assert_eq!(details[1].parents, [first]);
        assert_eq!(details[1].refs[0].kind, RefKind::Head);
    }

    #[test]
    fn details_at_most_fifty() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        for number in 0..55 {
            repo.git(&["commit", "-q", "--allow-empty", "-m", &number.to_string()]);
        }
        let store = store_of(&repo);
        let listing = repo.git(&["rev-list", "HEAD"]);
        let hashes: Vec<String> = listing.lines().map(str::to_string).collect();

        let details = commit_details(repo.path(), &store, &hashes).unwrap();

        assert_eq!(details.len(), 50);
        assert_eq!(details[49].hash, hashes[49]);
    }

    #[test]
    fn details_rejects_what_looks_like_an_option() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        let store = store_of(&repo);

        assert!(commit_details(repo.path(), &store, &["--all".to_string()]).is_err());
    }

    #[test]
    fn hash_in_message_that_exists_is_a_link() {
        let repo = TempRepo::new();
        let first = repo.commit("a.txt", b"1", "one");
        let short = &first[..9];
        let message = format!("reverts {short} and deadbee0");
        repo.commit("a.txt", b"2", &message);

        let details = one(&repo, &head(&repo));

        assert_eq!(details.links, BTreeMap::from([(short.to_string(), first)]));
    }

    #[test]
    fn merge_changes_are_those_against_all_parents() {
        let repo = merge_with_conflict();
        let store = store_of(&repo);

        let groups = commit_changes(repo.path(), &store, &head(&repo), ChangeMode::Merged).unwrap();

        assert_eq!(groups.len(), 1);
        assert!(groups[0].merge);
        assert_eq!(groups[0].parent, None);
        assert_eq!(paths(&groups[0]), ["a.txt"]);
        assert_eq!(groups[0].files[0].status, "M");
    }

    #[test]
    fn clean_merge_has_no_changes_of_its_own() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "base");
        repo.git(&["checkout", "-q", "-b", "other"]);
        repo.commit("b.txt", b"2", "on other");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("c.txt", b"3", "on main");
        repo.git(&["merge", "-q", "--no-ff", "-m", "merge", "other"]);
        let store = store_of(&repo);

        let groups = commit_changes(repo.path(), &store, &head(&repo), ChangeMode::Merged).unwrap();

        assert!(groups[0].merge);
        assert!(groups[0].files.is_empty());
    }

    #[test]
    fn changes_to_each_parent_carry_their_commit() {
        let repo = merge_with_conflict();
        let store = store_of(&repo);
        let first = repo.git(&["rev-parse", "HEAD^1"]).trim().to_string();
        let second = repo.git(&["rev-parse", "HEAD^2"]).trim().to_string();

        let groups =
            commit_changes(repo.path(), &store, &head(&repo), ChangeMode::Parents).unwrap();

        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].parent.as_ref(), Some(&first));
        assert_eq!(groups[0].base.as_ref(), Some(&first));
        assert_eq!(groups[0].subject.as_deref(), Some("c on main"));
        assert_eq!(paths(&groups[0]), ["a.txt", "b.txt"]);
        assert_eq!(groups[1].parent.as_ref(), Some(&second));
        assert_eq!(paths(&groups[1]), ["a.txt", "c.txt"]);
    }

    #[test]
    fn changes_of_a_normal_commit_and_of_the_first() {
        let repo = TempRepo::new();
        let first = repo.commit("a.txt", b"1", "one");
        let second = repo.commit("b.txt", b"2", "two");
        let store = store_of(&repo);

        let root = commit_changes(repo.path(), &store, &first, ChangeMode::Merged).unwrap();
        let next = commit_changes(repo.path(), &store, &second, ChangeMode::Parents).unwrap();

        assert_eq!(root[0].base, None);
        assert_eq!(paths(&root[0]), ["a.txt"]);
        assert_eq!(next.len(), 1);
        assert!(!next[0].merge);
        assert_eq!(next[0].parent, None);
        assert_eq!(next[0].base.as_ref(), Some(&first));
        assert_eq!(paths(&next[0]), ["b.txt"]);
    }

    #[test]
    fn different_statuses_are_joined_like_status_merger() {
        let file = |status: &str, path: &str, old: Option<&str>| ChangedFile {
            path: path.to_string(),
            old_path: old.map(str::to_string),
            status: status.to_string(),
        };
        let merged = merge_changes(vec![
            vec![
                file("M", "same", None),
                file("M", "mixed", None),
                file("D", "deleted", None),
                file("R100", "moved", Some("a")),
                file("R100", "two", Some("a")),
                file("A", "only", None),
            ],
            vec![
                file("M", "same", None),
                file("A", "mixed", None),
                file("M", "deleted", None),
                file("R090", "moved", Some("a")),
                file("R100", "two", Some("b")),
            ],
        ]);

        let found: Vec<_> = merged
            .iter()
            .map(|file| (&*file.path, &*file.status, file.old_path.as_deref()))
            .collect();
        assert_eq!(
            found,
            [
                ("same", "M", None),
                ("mixed", "M", None),
                ("deleted", "D", None),
                ("moved", "R100", Some("a")),
                ("two", "M", None),
            ]
        );
    }

    fn run_real(repo: &TempRepo) -> impl FnOnce(&[&str], &[u8]) -> Result<String, GitError> + '_ {
        |args, input| {
            let output = git::run_with_input(repo.path(), args, input)?;
            Ok(String::from_utf8_lossy(&output).into_owned())
        }
    }

    #[test]
    fn several_commits_come_out_of_one_command_with_their_group() {
        let repo = merge_with_conflict();
        let merge = head(&repo);
        let first = repo.git(&["rev-parse", "HEAD~1"]).trim().to_string();
        let root = repo
            .git(&["rev-list", "--max-parents=0", "HEAD"])
            .trim()
            .to_string();
        let hashes = [merge.clone(), first.clone(), root.clone()];

        let selected = selected_changes(&hashes, None, run_real(&repo)).unwrap();

        let order: Vec<_> = selected.commits.iter().map(|commit| &commit.hash).collect();
        assert_eq!(order, [&merge, &first, &root]);
        let [merged, plain, initial] = &selected.commits[..] else {
            panic!("three commits were expected");
        };
        assert!(merged.group.merge);
        assert_eq!(paths(&merged.group), ["a.txt"]);
        assert_eq!(merged.group.size, 4);
        assert_eq!(paths(&plain.group), ["c.txt"]);
        assert_eq!(initial.group.base, None);
        assert_eq!(paths(&initial.group), ["a.txt"]);
        assert_eq!(selected.max_size, 4);
    }

    #[test]
    fn batched_same_as_commit_by_commit() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"enough content to detect the change", "base");
        repo.git(&["mv", "a.txt", "with space and ñ.txt"]);
        repo.git(&["commit", "-q", "-m", "renames"]);
        let hash = head(&repo);

        let selected = selected_changes(&[hash], None, run_real(&repo)).unwrap();

        let file = &selected.commits[0].group.files[0];
        assert_eq!(file.path, "with space and ñ.txt");
        assert_eq!(file.old_path.as_deref(), Some("a.txt"));
        assert!(file.status.starts_with('R'));
    }

    #[test]
    fn merge_equal_to_a_parent_has_no_changes_of_its_own() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "base");
        repo.git(&["checkout", "-q", "-b", "other"]);
        repo.commit("b.txt", b"2", "on other");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("c.txt", b"3", "on main");
        repo.git(&[
            "merge", "-q", "--no-ff", "-s", "ours", "-m", "merge", "other",
        ]);
        let store = store_of(&repo);
        let hash = head(&repo);

        let selected =
            selected_changes(std::slice::from_ref(&hash), None, run_real(&repo)).unwrap();
        let single = commit_changes(repo.path(), &store, &hash, ChangeMode::Merged).unwrap();

        assert!(selected.commits[0].group.merge);
        assert!(selected.commits[0].group.files.is_empty());
        assert!(single[0].files.is_empty());
        assert_eq!(selected.commits[0].group.size, single[0].size);
    }

    #[test]
    fn above_the_limit_no_file_travels() {
        let repo = merge_with_conflict();
        let hash = head(&repo);

        let selected = selected_changes(&[hash], Some(3), run_real(&repo)).unwrap();

        assert_eq!(selected.max_size, 4);
        assert!(selected.commits.is_empty());
    }

    #[test]
    fn the_limit_is_that_of_the_largest_commit_not_the_sum_of_all() {
        let repo = TempRepo::new();
        let first = repo.commit("a.txt", b"1", "one");
        let second = repo.commit("b.txt", b"2", "two");

        let selected = selected_changes(&[second, first], Some(1), run_real(&repo)).unwrap();

        assert_eq!(selected.max_size, 1);
        assert_eq!(selected.commits.len(), 2);
    }

    #[test]
    fn batched_rejects_what_looks_like_an_option() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");

        assert!(selected_changes(&["--all".to_string()], None, run_real(&repo)).is_err());
    }

    #[test]
    fn commit_size_adds_up_all_its_parents() {
        let repo = merge_with_conflict();
        let store = store_of(&repo);
        let hash = head(&repo);

        let merged = commit_changes(repo.path(), &store, &hash, ChangeMode::Merged).unwrap();
        let root = repo
            .git(&["rev-list", "--max-parents=0", "HEAD"])
            .trim()
            .to_string();
        let initial = commit_changes(repo.path(), &store, &root, ChangeMode::Merged).unwrap();

        assert_eq!(merged[0].size, 4);
        assert_eq!(initial[0].size, 1);
    }

    #[test]
    fn containing_branches_in_memory() {
        let repo = TempRepo::new();
        let base = repo.commit("a.txt", b"1", "base");
        repo.git(&["tag", "v1"]);
        repo.git(&["checkout", "-q", "-b", "other"]);
        let side = repo.commit("b.txt", b"2", "side");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("c.txt", b"3", "main");

        assert_eq!(containing(&repo, &base), ["HEAD", "main", "other"]);
        assert_eq!(containing(&repo, &side), ["other"]);
        assert!(containing(&repo, &"0".repeat(40)).is_empty());
    }

    #[test]
    fn change_mode_comes_from_the_frontend() {
        let merged: ChangeMode = serde_json::from_str("\"merged\"").unwrap();
        let parents: ChangeMode = serde_json::from_str("\"parents\"").unwrap();

        assert_eq!((merged, parents), (ChangeMode::Merged, ChangeMode::Parents));
    }
}
