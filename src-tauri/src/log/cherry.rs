// `Not Cherry-Picked Commits` of the log's View Options (`DeepCompareAction`, `DeepComparator`).
// Without IntelliJ's index, 241 asks `git cherry`, which matches commits by patch only (overview
// decision 23): a commit picked with another patch is not recognised.
use serde::Serialize;

use crate::diff::check_hash;
use crate::git::GitError;
use crate::log::filter::{Highlighter, LogFilter};
use crate::log::store::CommitStore;

// What a comparison found: the current branch it was made against (`HEAD` when detached), and
// the commits of the compared branch that have no equivalent in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotPicked {
    pub current: String,
    pub commits: Vec<String>,
}

// `DeepComparator.getComparedBranchFromFilters`: the only branch of the Branch filter, or else the
// inclusive end of its only range.
pub fn compared_branch(store: &CommitStore, filter: &LogFilter) -> Option<String> {
    if let Some(branch) = Highlighter::single_branch(store, filter) {
        return Some(branch.to_string());
    }
    match &filter.ranges[..] {
        [(_, inclusive)] => Some(inclusive.clone()),
        _ => None,
    }
}

// `getCommitsFromGit`: `git cherry <current> <compared>`, whose `+` lines are the commits of
// `compared` with no equivalent patch in `current`. Both are full hashes, resolved before: no name
// that came from the page reaches git, where it could be read as an option.
pub fn not_picked(
    git: &mut dyn FnMut(&[&str]) -> Result<String, GitError>,
    current: &str,
    compared: &str,
) -> Result<Vec<String>, GitError> {
    check_hash(current)?;
    check_hash(compared)?;
    let output = git(&["cherry", current, compared])?;
    Ok(output
        .lines()
        .filter_map(|line| line.strip_prefix('+'))
        // `safety-check: take just the first word for sure`.
        .filter_map(|rest| rest.split_whitespace().next())
        .map(str::to_string)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git;
    use crate::log::store;
    use crate::test_support::TempRepo;

    fn run(repo: &TempRepo) -> impl FnMut(&[&str]) -> Result<String, GitError> + '_ {
        move |args| Ok(String::from_utf8_lossy(&git::run(repo.path(), args)?).into_owned())
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn cherry_marks_picked_commits_by_patch() {
        let repo = TempRepo::new();
        repo.commit("base.txt", b"base\n", "base");
        repo.git(&["checkout", "-q", "-b", "feature"]);
        let picked = repo.commit("picked.txt", b"picked\n", "picked");
        let left = repo.commit("left.txt", b"left\n", "left");
        // Picked again with another message: the patch is the same.
        let reworded = repo.commit("reworded.txt", b"reworded\n", "reworded");
        let feature = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("main.txt", b"main\n", "main");
        repo.git(&["cherry-pick", &picked]);
        repo.git(&["cherry-pick", "--no-commit", &reworded]);
        repo.git(&["commit", "-q", "-m", "another message"]);
        // The same file with other content is another patch, and is not taken as picked.
        repo.commit("left.txt", b"changed\n", "like left");
        let main = repo.git(&["rev-parse", "HEAD"]).trim().to_string();

        let found = not_picked(&mut run(&repo), &main, &feature).unwrap();

        assert_eq!(found, [left]);
    }

    #[test]
    fn only_full_hashes_reach_git() {
        let repo = TempRepo::new();
        let head = repo.commit("a.txt", b"a\n", "a");
        let mut asked = Vec::new();
        let mut git = |args: &[&str]| {
            asked.push(args.join(" "));
            Ok(String::new())
        };

        for (current, compared) in [("--all", head.as_str()), (head.as_str(), "main"), ("", "")] {
            assert!(not_picked(&mut git, current, compared).is_err());
        }

        assert!(asked.is_empty());
    }

    #[test]
    fn the_compared_branch_is_the_only_branch_or_the_end_of_the_only_range() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"a\n", "a");
        repo.git(&["branch", "feature"]);
        repo.git(&["branch", "fix"]);
        let store = store::load(repo.path()).unwrap();
        let compared = |filter: LogFilter| compared_branch(&store, &filter);

        let branches = |values: &[&str]| LogFilter {
            branches: strings(values),
            ..LogFilter::default()
        };
        assert_eq!(
            compared(branches(&["feature"])),
            Some("feature".to_string())
        );
        assert_eq!(compared(branches(&["fe.*"])), Some("feature".to_string()));
        assert_eq!(compared(branches(&["f.*"])), None);
        assert_eq!(compared(branches(&["feature", "fix"])), None);
        assert_eq!(compared(LogFilter::default()), None);
        let range = |pairs: &[(&str, &str)]| LogFilter {
            ranges: pairs
                .iter()
                .map(|(from, to)| (from.to_string(), to.to_string()))
                .collect(),
            ..LogFilter::default()
        };
        assert_eq!(compared(range(&[("main", "fix")])), Some("fix".to_string()));
        assert_eq!(compared(range(&[("main", "fix"), ("fix", "main")])), None);
    }
}
