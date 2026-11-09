// A file history in the Log: the commits of `history::file_history` on the permanent graph, with
// dotted edges between them, as `FileHistoryFilterer.filterWithVcs` builds it when the log is full
// (rama's always is), and the path of the file in each of them (`FileHistory`, `FileHistoryPaths`).

use std::collections::HashMap;
use std::sync::Arc;

use serde::Serialize;

use crate::history::HistoryRow;
use crate::log::filter::{self, BitVec, Selection};
use crate::log::store::CommitStore;
use crate::log::view::{Permanent, View, ViewSpec};

// The file in one commit of the history: its path there and whether that commit deleted it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub path: String,
    pub deleted: bool,
}

// Per commit of the store, its entry. A commit the log has not read (one from after it was
// loaded) has none, and no row.
#[derive(Debug, Default)]
pub struct HistoryPaths {
    entries: HashMap<u32, HistoryEntry>,
}

pub fn history_view(
    store: &CommitStore,
    permanent: Arc<Permanent>,
    spec: &ViewSpec,
    rows: &[HistoryRow],
) -> (View, HistoryPaths) {
    let found = store.find(rows.iter().map(|row| row.hash.as_str()));
    let mut kept = BitVec::new(store.len() as usize);
    let mut paths = HistoryPaths::default();
    for row in rows {
        let Some(&commit) = found.get(row.hash.as_str()) else {
            continue;
        };
        kept.set(commit);
        paths.entries.entry(commit).or_insert_with(|| HistoryEntry {
            path: row.path.clone(),
            deleted: row.deleted(),
        });
    }
    let view = View::filtered(permanent, spec, Selection::Commits(Arc::new(kept)));
    (view, paths)
}

impl HistoryPaths {
    pub fn of_commit(&self, commit: u32) -> Option<&HistoryEntry> {
        self.entries.get(&commit)
    }
}

// `FileHistoryPaths.filePath` of the row's commit.
pub fn history_path<'a>(
    view: &View,
    paths: &'a HistoryPaths,
    row: u32,
) -> Option<&'a HistoryEntry> {
    let commit = *view.graph.rows.get(row as usize)?;
    paths.of_commit(commit)
}

// One side of a history diff: a commit and the path the file has in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistorySide {
    pub hash: String,
    pub path: String,
}

// `FileHistoryUtil.createChangeToParents`: the row against each parent row. `after` is missing where
// the row deleted the file, a `before` where a parent did or has none. Two or more `before` are a
// merge (`MergedChange`, three sides with two parents); none is the file's first commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryChange {
    pub hash: String,
    pub path: String,
    pub after: Option<HistorySide>,
    pub before: Vec<Option<HistorySide>>,
}

fn side(store: &CommitStore, paths: &HistoryPaths, view: &View, row: u32) -> Option<HistorySide> {
    let commit = *view.graph.rows.get(row as usize)?;
    let entry = paths.of_commit(commit).filter(|entry| !entry.deleted)?;
    Some(HistorySide {
        hash: store.get(commit).hash.to_string(),
        path: entry.path.clone(),
    })
}

// The rows a merge row goes down to, in the order of the merge's parents (`MyVcsChangesMerger`
// takes the first parent first): each one under the first parent it is reachable from. A dotted
// edge goes to an ancestor of a parent, not to the parent itself.
fn in_parent_order(store: &CommitStore, view: &View, commit: u32, downs: Vec<u32>) -> Vec<u32> {
    if downs.len() < 2 {
        return downs;
    }
    let reach: Vec<BitVec> = store
        .get(commit)
        .parents
        .iter()
        .map(|parent| filter::reachable(store, &[*parent]))
        .collect();
    let mut ordered: Vec<(usize, u32)> = downs
        .into_iter()
        .map(|down| {
            let target = view.graph.rows[down as usize];
            let parent = reach.iter().position(|bits| bits.get(target));
            (parent.unwrap_or(usize::MAX), down)
        })
        .collect();
    ordered.sort();
    ordered.into_iter().map(|(_, down)| down).collect()
}

// `FileHistoryModel.getSelectedChange`: one row against the rows its edges go down to (the dotted
// edges included), several rows the newest against the oldest. `rows` go from the top.
pub fn history_change(
    store: &CommitStore,
    view: &View,
    paths: &HistoryPaths,
    rows: &[u32],
) -> Option<HistoryChange> {
    let (&row, rest) = rows.split_first()?;
    let commit = *view.graph.rows.get(row as usize)?;
    let entry = paths.of_commit(commit)?;
    let parents: Vec<u32> = match rest.last() {
        Some(&oldest) => vec![oldest],
        None => {
            let mut downs: Vec<u32> = view
                .graph
                .downs
                .of(row)
                .iter()
                .map(|edge| edge.down)
                .collect();
            downs.sort_unstable();
            downs.dedup();
            in_parent_order(store, view, commit, downs)
        }
    };
    let after = side(store, paths, view, row);
    let before: Vec<Option<HistorySide>> = if parents.is_empty() {
        after.as_ref()?;
        Vec::new()
    } else {
        let befores: Vec<Option<HistorySide>> = parents
            .iter()
            .map(|parent| side(store, paths, view, *parent))
            .filter(|before| before.is_some() || after.is_some())
            .collect();
        if befores.is_empty() {
            return None;
        }
        befores
    };
    Some(HistoryChange {
        hash: store.get(commit).hash.to_string(),
        path: entry.path.clone(),
        after,
        before,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git;
    use crate::history::{self, HistoryRow};
    use crate::log::graph::EdgeStyle;
    use crate::log::store;
    use crate::test_support::TempRepo;

    struct Built {
        store: CommitStore,
        view: View,
        paths: HistoryPaths,
    }

    fn built(repo: &TempRepo, path: &str) -> Built {
        let store = store::load(repo.path()).unwrap();
        let mut run = |args: &[&str]| {
            git::run(repo.path(), args).map(|out| String::from_utf8_lossy(&out).into_owned())
        };
        let rows = history::file_history(
            &mut run,
            path,
            &history::Starts::Revisions(vec!["HEAD".to_string()]),
            history::Walk::Full,
        )
        .unwrap();
        let permanent = Arc::new(Permanent::from_store(&store, store.refs()));
        let (view, paths) = history_view(&store, permanent, &ViewSpec::default(), &rows);
        Built { store, view, paths }
    }

    fn hash_at(built: &Built, row: u32) -> String {
        built
            .store
            .get(built.view.graph.rows[row as usize])
            .hash
            .to_string()
    }

    fn rev(repo: &TempRepo, name: &str) -> String {
        repo.git(&["rev-parse", name]).trim().to_string()
    }

    fn side_of(hash: &str, path: &str) -> Option<HistorySide> {
        Some(HistorySide {
            hash: hash.to_string(),
            path: path.to_string(),
        })
    }

    fn text(extra: &str) -> Vec<u8> {
        let mut text: String = (1..=20).map(|line| format!("line {line}\n")).collect();
        text.push_str(extra);
        text.into_bytes()
    }

    #[test]
    fn rows_are_the_history_commits_joined_by_dotted_edges_with_their_paths() {
        let repo = TempRepo::new();
        let added = repo.commit("old.txt", &text(""), "add");
        repo.commit("other.txt", b"1\n", "unrelated");
        repo.git(&["mv", "old.txt", "new.txt"]);
        repo.git(&["commit", "-q", "-m", "rename"]);
        let renamed = rev(&repo, "HEAD");

        let built = built(&repo, "new.txt");

        assert_eq!(built.view.graph.len(), 2);
        assert_eq!(hash_at(&built, 0), renamed);
        assert_eq!(hash_at(&built, 1), added);
        let edges = built.view.graph.downs.of(0);
        assert_eq!(edges.len(), 1);
        assert_eq!((edges[0].down, edges[0].style), (1, EdgeStyle::Dotted));
        let path = |row| {
            history_path(&built.view, &built.paths, row)
                .unwrap()
                .path
                .clone()
        };
        assert_eq!(
            (path(0), path(1)),
            ("new.txt".to_string(), "old.txt".to_string())
        );
    }

    #[test]
    fn a_row_compares_with_its_parent_row_at_the_path_there() {
        let repo = TempRepo::new();
        let added = repo.commit("old.txt", &text(""), "add");
        repo.git(&["mv", "old.txt", "new.txt"]);
        repo.git(&["commit", "-q", "-m", "rename"]);
        let renamed = rev(&repo, "HEAD");

        let built = built(&repo, "new.txt");

        let change = history_change(&built.store, &built.view, &built.paths, &[0]).unwrap();
        assert_eq!(change.after, side_of(&renamed, "new.txt"));
        assert_eq!(change.before, vec![side_of(&added, "old.txt")]);
        let first = history_change(&built.store, &built.view, &built.paths, &[1]).unwrap();
        assert_eq!(first.before, Vec::new());
    }

    #[test]
    fn two_rows_compare_the_newest_with_the_oldest() {
        let repo = TempRepo::new();
        let first = repo.commit("a.txt", b"1\n", "one");
        repo.commit("a.txt", b"2\n", "two");
        let last = repo.commit("a.txt", b"3\n", "three");

        let built = built(&repo, "a.txt");

        let change = history_change(&built.store, &built.view, &built.paths, &[0, 1, 2]).unwrap();
        assert_eq!(change.after, side_of(&last, "a.txt"));
        assert_eq!(change.before, vec![side_of(&first, "a.txt")]);
    }

    // Writes `contents` and commits it with that date for author and committer: the rows follow it.
    fn commit_at(repo: &TempRepo, contents: &[u8], message: &str, date: &str) -> String {
        std::fs::write(repo.path().join("a.txt"), contents).unwrap();
        repo.git(&["add", "a.txt"]);
        let status = crate::test_support::git_command()
            .args(["commit", "-q", "-m", message])
            .current_dir(repo.path())
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .status()
            .unwrap();
        assert!(status.success());
        rev(repo, "HEAD")
    }

    // `MergedChange` (`FileHistoryUtil.createChangeToParents`): a merge that both sides changed is
    // kept by `--simplify-merges`, and its row goes down to both parents, the first parent first
    // (`MyVcsChangesMerger`), even when the second one is newer and its row is above.
    #[test]
    fn a_merge_row_opens_three_sides() {
        let repo = TempRepo::new();
        let base = String::from_utf8(text("")).unwrap();
        commit_at(&repo, base.as_bytes(), "base", "2026-01-01T12:00:00");
        repo.git(&["checkout", "-q", "-b", "side"]);
        repo.git(&["checkout", "-q", "main"]);
        let ours = commit_at(
            &repo,
            format!("{base}main\n").as_bytes(),
            "main",
            "2026-01-02T12:00:00",
        );
        repo.git(&["checkout", "-q", "side"]);
        let theirs = commit_at(
            &repo,
            format!("side\n{base}").as_bytes(),
            "side",
            "2026-01-03T12:00:00",
        );
        repo.git(&["checkout", "-q", "main"]);
        repo.git(&["merge", "-q", "--no-ff", "-m", "merge", "side"]);
        let merge = rev(&repo, "HEAD");

        let built = built(&repo, "a.txt");

        assert_eq!(hash_at(&built, 0), merge);
        assert_eq!(hash_at(&built, 1), theirs);
        let change = history_change(&built.store, &built.view, &built.paths, &[0]).unwrap();
        assert_eq!(change.after, side_of(&merge, "a.txt"));
        let before: Vec<String> = change
            .before
            .iter()
            .map(|side| side.clone().unwrap().hash)
            .collect();
        assert_eq!(before, vec![ours, theirs]);
    }

    #[test]
    fn a_deleted_row_has_no_after_side() {
        let repo = TempRepo::new();
        let added = repo.commit("a.txt", b"1\n", "one");
        repo.git(&["rm", "-q", "a.txt"]);
        repo.git(&["commit", "-q", "-m", "delete"]);
        let deleted = rev(&repo, "HEAD");

        let built = built(&repo, "a.txt");

        assert_eq!(hash_at(&built, 0), deleted);
        assert!(history_path(&built.view, &built.paths, 0).unwrap().deleted);
        let change = history_change(&built.store, &built.view, &built.paths, &[0]).unwrap();
        assert_eq!(change.after, None);
        assert_eq!(change.before, vec![side_of(&added, "a.txt")]);
    }

    #[test]
    fn a_commit_the_log_has_not_read_has_no_row() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");
        let store = store::load(repo.path()).unwrap();
        let permanent = Arc::new(Permanent::from_store(&store, store.refs()));
        let unknown = HistoryRow {
            hash: "f".repeat(40),
            path: "a.txt".to_string(),
            status: "M".to_string(),
        };

        let (view, paths) = history_view(&store, permanent, &ViewSpec::default(), &[unknown]);

        assert_eq!(view.graph.len(), 0);
        assert!(paths.entries.is_empty());
    }
}
