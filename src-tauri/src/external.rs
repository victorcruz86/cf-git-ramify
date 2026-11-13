// Ramify's own `Notify about Git changes made outside Ramify` (Settings > Ramify,
// `docs/parity-matrix.md`). Another program (a terminal, Claude Code) switched the branch, moved
// HEAD off its commit, or created or took a stash while the repository was open: the window is told
// what changed. Commits, merges and pulls that keep the old HEAD are left out: nothing is lost.
//
// Each open repository has the state Ramify last knew. Every operation of Ramify runs under
// `RepoLocks` and records the state before it lets go, so what it did is never someone else's; the
// watcher compares under the same lock, and skips while an operation holds it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde::Serialize;

use crate::git;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Snapshot {
    // `None` with a detached HEAD.
    pub branch: Option<String>,
    // `None` before the first commit.
    pub head: Option<String>,
    // Newest first, by hash, with the message `git stash list` shows.
    pub stash: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ExternalChange {
    // A detached HEAD is named by its short hash.
    BranchSwitched { from: String, to: String },
    // On the same branch (or detached), to a commit that does not contain the old one: a reset, a
    // rebase or an amend. `to` is the full hash, for `Show in Log`.
    HeadMoved { from: String, to: String },
    StashCreated { message: String },
    // `git stash pop`, `apply` + `drop`, or `drop`: the reflog does not tell them apart.
    StashRemoved { message: String },
}

const SHORT: usize = 8;

fn short(hash: &str) -> String {
    hash.chars().take(SHORT).collect()
}

fn text(output: Vec<u8>) -> String {
    String::from_utf8_lossy(&output).trim().to_string()
}

pub fn read(repo: &Path) -> Snapshot {
    // Both fail on purpose: `symbolic-ref` with a detached HEAD, `rev-parse` without commits.
    let branch = git::run(repo, &["symbolic-ref", "-q", "--short", "HEAD"])
        .ok()
        .map(text)
        .filter(|name| !name.is_empty());
    let head = git::run(repo, &["rev-parse", "-q", "--verify", "HEAD^{commit}"])
        .ok()
        .map(text)
        .filter(|hash| !hash.is_empty());
    let stash = git::run(repo, &["stash", "list", "--format=%H%x1f%gs"])
        .map(|output| {
            String::from_utf8_lossy(&output)
                .lines()
                .filter_map(|line| line.split_once('\u{1f}'))
                .map(|(hash, message)| (hash.to_string(), message.to_string()))
                .collect()
        })
        .unwrap_or_default();
    Snapshot {
        branch,
        head,
        stash,
    }
}

fn name(snapshot: &Snapshot) -> String {
    match (&snapshot.branch, &snapshot.head) {
        (Some(branch), _) => branch.clone(),
        (None, Some(head)) => short(head),
        (None, None) => "HEAD".to_string(),
    }
}

// `contains(old, new)`: whether `new` has `old` in its history.
pub fn changes(
    old: &Snapshot,
    new: &Snapshot,
    contains: impl Fn(&str, &str) -> bool,
) -> Vec<ExternalChange> {
    let mut found = Vec::new();
    if old.branch != new.branch {
        found.push(ExternalChange::BranchSwitched {
            from: name(old),
            to: name(new),
        });
    } else if let (Some(from), Some(to)) = (&old.head, &new.head) {
        if from != to && !contains(from, to) {
            found.push(ExternalChange::HeadMoved {
                from: short(from),
                to: to.clone(),
            });
        }
    }
    let had = |list: &[(String, String)], hash: &str| list.iter().any(|(known, _)| known == hash);
    for (hash, message) in new.stash.iter().rev() {
        if !had(&old.stash, hash) {
            found.push(ExternalChange::StashCreated {
                message: message.clone(),
            });
        }
    }
    for (hash, message) in &old.stash {
        if !had(&new.stash, hash) {
            found.push(ExternalChange::StashRemoved {
                message: message.clone(),
            });
        }
    }
    found
}

fn contains_in(repo: &Path) -> impl Fn(&str, &str) -> bool + '_ {
    move |old, new| git::run(repo, &["merge-base", "--is-ancestor", old, new]).is_ok()
}

static KNOWN: Mutex<Option<HashMap<PathBuf, Snapshot>>> = Mutex::new(None);

fn with_known<T>(work: impl FnOnce(&mut HashMap<PathBuf, Snapshot>) -> T) -> T {
    let mut known = KNOWN.lock().unwrap_or_else(PoisonError::into_inner);
    work(known.get_or_insert_with(HashMap::new))
}

// A repository opened in a window starts from what it is now.
pub fn track(repo: &Path) {
    let now = read(repo);
    with_known(|known| known.insert(repo.to_path_buf(), now));
}

// After an operation of Ramify, still under its lock. A repository no window opened is not read.
pub fn record(repo: &Path) {
    if !with_known(|known| known.contains_key(repo)) {
        return;
    }
    let now = read(repo);
    with_known(|known| known.insert(repo.to_path_buf(), now));
}

// What changed since Ramify last knew; it then knows the new state. To be called under the
// repository's lock, so that no operation of Ramify runs meanwhile.
pub fn check(repo: &Path) -> Vec<ExternalChange> {
    let now = read(repo);
    let before = with_known(|known| known.insert(repo.to_path_buf(), now.clone()));
    before.map_or_else(Vec::new, |before| changes(&before, &now, contains_in(repo)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::RepoLocks;
    use crate::test_support::TempRepo;
    use std::fs;

    fn snapshot(branch: Option<&str>, head: Option<&str>, stash: &[(&str, &str)]) -> Snapshot {
        Snapshot {
            branch: branch.map(str::to_string),
            head: head.map(str::to_string),
            stash: stash
                .iter()
                .map(|(hash, message)| (hash.to_string(), message.to_string()))
                .collect(),
        }
    }

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn a_new_commit_on_the_same_branch_is_not_reported() {
        let old = snapshot(Some("main"), Some(A), &[]);
        let new = snapshot(Some("main"), Some(B), &[]);

        assert!(changes(&old, &new, |_, _| true).is_empty());
        assert_eq!(
            changes(&old, &new, |_, _| false),
            [ExternalChange::HeadMoved {
                from: "aaaaaaaa".into(),
                to: B.into()
            }]
        );
    }

    #[test]
    fn a_switch_is_one_change_and_a_detached_head_is_named_by_its_hash() {
        let old = snapshot(Some("main"), Some(A), &[]);
        let new = snapshot(None, Some(B), &[]);

        assert_eq!(
            changes(&old, &new, |_, _| false),
            [ExternalChange::BranchSwitched {
                from: "main".into(),
                to: "bbbbbbbb".into()
            }]
        );
    }

    #[test]
    fn stashes_are_compared_by_hash_not_by_position() {
        let old = snapshot(Some("main"), Some(A), &[("s1", "On main: one")]);
        let pushed = snapshot(
            Some("main"),
            Some(A),
            &[("s2", "On main: two"), ("s1", "On main: one")],
        );
        let popped = snapshot(Some("main"), Some(A), &[("s2", "On main: two")]);

        assert_eq!(
            changes(&old, &pushed, |_, _| true),
            [ExternalChange::StashCreated {
                message: "On main: two".into()
            }]
        );
        assert_eq!(
            changes(&pushed, &popped, |_, _| true),
            [ExternalChange::StashRemoved {
                message: "On main: one".into()
            }]
        );
    }

    fn changed_files(repo: &TempRepo) {
        fs::write(repo.path().join("env.js"), "local\n").unwrap();
    }

    // The session that showed the problem: `git stash -q && tsc ; git stash pop -q` in a terminal.
    #[test]
    fn a_stash_and_pop_from_a_terminal_are_both_reported() {
        let repo = TempRepo::new();
        repo.commit("env.js", b"one\n", "first");
        changed_files(&repo);
        track(repo.path());

        repo.git(&["stash", "-q"]);
        let pushed = check(repo.path());
        repo.git(&["stash", "pop", "-q"]);
        let popped = check(repo.path());

        assert!(
            matches!(pushed.as_slice(), [ExternalChange::StashCreated { message }] if message.starts_with("WIP on main:")),
            "{pushed:?}"
        );
        assert!(matches!(popped.as_slice(), [ExternalChange::StashRemoved { .. }]));
        assert!(check(repo.path()).is_empty());
    }

    #[test]
    fn checkout_and_reset_from_a_terminal_are_reported_and_a_commit_is_not() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\n", "first");
        track(repo.path());

        repo.commit("a.txt", b"two\n", "second");
        assert!(check(repo.path()).is_empty());

        repo.git(&["reset", "-q", "--hard", "HEAD~1"]);
        assert!(matches!(check(repo.path()).as_slice(), [ExternalChange::HeadMoved { .. }]));

        repo.git(&["checkout", "-q", "-b", "side"]);
        assert_eq!(
            check(repo.path()),
            [ExternalChange::BranchSwitched {
                from: "main".into(),
                to: "side".into()
            }]
        );
    }

    #[test]
    fn what_ramify_does_under_its_lock_is_not_reported() {
        let repo = TempRepo::new();
        repo.commit("env.js", b"one\n", "first");
        changed_files(&repo);
        track(repo.path());
        let locks = RepoLocks::default();

        locks.run(repo.path(), || repo.git(&["stash", "-q"]));
        locks.run(repo.path(), || repo.git(&["checkout", "-q", "-b", "side"]));

        assert!(check(repo.path()).is_empty());
    }

    #[test]
    fn a_repository_no_window_opened_is_not_read_after_an_operation() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"one\n", "first");

        record(repo.path());

        assert!(!with_known(|known| known.contains_key(repo.path())));
    }
}
