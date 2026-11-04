use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;

use serde::Serialize;

use crate::git::{self, GitError};
use crate::log::parse::{parse_record, LOG_FORMAT};
use crate::log::refs::{parse_decoration, GitRef, Refs};

pub struct Commit {
    pub hash: Box<str>,
    pub parents: Box<[u32]>,
    pub author: u32,
    pub committer: u32,
    pub author_time: i64,
    pub commit_time: i64,
    pub subject: Box<str>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Person {
    pub name: Box<str>,
    pub email: Box<str>,
}

// Each author or committer is stored once, however many commits they sign.
#[derive(Default)]
pub struct People {
    list: Vec<Person>,
}

impl People {
    pub fn len(&self) -> u32 {
        self.list.len() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn get(&self, index: u32) -> &Person {
        &self.list[index as usize]
    }

    pub fn all(&self) -> &[Person] {
        &self.list
    }
}

#[derive(Default)]
pub struct CommitStore {
    commits: Vec<Commit>,
    people: People,
    refs: Refs,
}

impl CommitStore {
    pub fn len(&self) -> u32 {
        self.commits.len() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.commits.is_empty()
    }

    pub fn get(&self, index: u32) -> &Commit {
        &self.commits[index as usize]
    }

    // Walks the whole store: it is rarely requested, and an index would always cost memory.
    pub fn find<'a>(&self, hashes: impl Iterator<Item = &'a str>) -> HashMap<&'a str, u32> {
        let mut wanted: HashMap<&str, &'a str> = hashes.map(|hash| (hash, hash)).collect();
        let mut found = HashMap::with_capacity(wanted.len());
        for (index, commit) in self.commits.iter().enumerate() {
            if wanted.is_empty() {
                break;
            }
            if let Some(hash) = wanted.remove(&*commit.hash) {
                found.insert(hash, index as u32);
            }
        }
        found
    }

    pub fn with_prefix(&self, prefix: &str) -> Option<&str> {
        self.commits
            .iter()
            .map(|commit| &*commit.hash)
            .find(|hash| hash.starts_with(prefix))
    }

    pub fn parents(&self) -> impl Iterator<Item = &[u32]> {
        self.commits.iter().map(|commit| &*commit.parents)
    }

    pub fn people(&self) -> &People {
        &self.people
    }

    pub fn refs(&self) -> &Refs {
        &self.refs
    }
}

// What is kept of each commit while the others arrive: the parents are still hashes
// because in log order they come later.
struct Pending {
    hash: String,
    parents: Vec<String>,
    author: u32,
    committer: u32,
    author_time: i64,
    commit_time: i64,
    subject: String,
}

#[derive(Default)]
struct Interner {
    people: People,
    index: HashMap<String, u32>,
    key: String,
}

impl Interner {
    fn intern(&mut self, name: String, email: String) -> u32 {
        self.key.clear();
        self.key.push_str(&name);
        self.key.push('\0');
        self.key.push_str(&email);
        if let Some(index) = self.index.get(&self.key) {
            return *index;
        }
        let index = self.people.list.len() as u32;
        self.people.list.push(Person {
            name: name.into(),
            email: email.into(),
        });
        self.index.insert(self.key.clone(), index);
        index
    }
}

// Like `GitLogProvider.readAllHashes`: `HEAD --branches --remotes --tags`, which unlike
// `--all` leaves out the stash. With `log.showSignature` in the user's configuration, gpg's
// output would arrive mixed with the records.
pub fn load(repo: &Path) -> Result<CommitStore, GitError> {
    let mut args = vec![
        "-c",
        "log.showSignature=false",
        "log",
        "--date-order",
        "--decorate=full",
        "--encoding=UTF-8",
        "-z",
        LOG_FORMAT,
    ];
    if has_head(repo)? {
        args.push("HEAD");
    }
    args.extend(["--branches", "--remotes", "--tags", "--"]);
    let mut pending: Vec<Pending> = Vec::new();
    let mut interner = Interner::default();
    let mut refs = Refs::default();
    let result = git::stream(repo, &args, 0, |record| {
        let Some(raw) = parse_record(record) else {
            return;
        };
        let commit = pending.len() as u32;
        for item in parse_decoration(&raw.refs) {
            if item.current {
                refs.current = Some(refs.all.len() as u32);
            }
            refs.all.push(GitRef {
                name: item.name.into(),
                kind: item.kind,
                commit,
            });
        }
        pending.push(Pending {
            author: interner.intern(raw.author_name, raw.author_email),
            committer: interner.intern(raw.committer_name, raw.committer_email),
            hash: raw.hash,
            parents: raw.parents,
            author_time: raw.author_time,
            commit_time: raw.commit_time,
            subject: raw.subject,
        });
    });
    match result {
        Err(GitError::Failed { stderr, .. }) if stderr.contains("does not have any commits") => {
            return Ok(CommitStore {
                commits: Vec::new(),
                people: People::default(),
                refs: Refs::default(),
            });
        }
        Err(error) => return Err(error),
        Ok(()) => {}
    }
    refs.track(&upstreams(repo)?);
    Ok(CommitStore {
        commits: resolve(pending),
        people: interner.people,
        refs,
    })
}

fn has_head(repo: &Path) -> Result<bool, GitError> {
    match git::run(repo, &["rev-parse", "--verify", "-q", "HEAD"]) {
        Ok(_) => Ok(true),
        Err(GitError::Failed { .. }) => Ok(false),
        Err(error) => Err(error),
    }
}

// Each local branch with the remote one it tracks according to its configuration, without the
// `refs/` prefixes.
fn upstreams(repo: &Path) -> Result<HashMap<String, String>, GitError> {
    let output = git::run(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname)%09%(upstream)",
            "refs/heads",
        ],
    )?;
    let text = String::from_utf8_lossy(&output);
    Ok(text
        .lines()
        .filter_map(|line| {
            let (branch, upstream) = line.split_once('\t')?;
            Some((
                branch.strip_prefix("refs/heads/")?.to_string(),
                upstream.strip_prefix("refs/remotes/")?.to_string(),
            ))
        })
        .collect())
}

// Missing parents, as in a shallow clone, are discarded.
fn resolve(pending: Vec<Pending>) -> Vec<Commit> {
    let indices: HashMap<&str, u32> = pending
        .iter()
        .enumerate()
        .map(|(index, commit)| (commit.hash.as_str(), index as u32))
        .collect();
    let parents: Vec<Box<[u32]>> = pending
        .iter()
        .map(|commit| {
            commit
                .parents
                .iter()
                .filter_map(|parent| indices.get(parent.as_str()).copied())
                .collect()
        })
        .collect();
    drop(indices);

    pending
        .into_iter()
        .zip(parents)
        .map(|(commit, parents)| Commit {
            hash: commit.hash.into(),
            parents,
            author: commit.author,
            committer: commit.committer,
            author_time: commit.author_time,
            commit_time: commit.commit_time,
            subject: commit.subject.into(),
        })
        .collect()
}

// It changes if anything the log reads changes: the branches, remotes and tags with their commit
// and their tracking, or where `HEAD` points. Editing files or stashing does not move it.
pub fn refs_fingerprint(repo: &Path) -> Result<u64, GitError> {
    let listing = git::run(
        repo,
        &[
            "for-each-ref",
            "--format=%(objectname) %(refname) %(upstream)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ],
    )?;
    let mut hasher = DefaultHasher::new();
    listing.hash(&mut hasher);
    optional_output(repo, &["symbolic-ref", "-q", "HEAD"])?.hash(&mut hasher);
    optional_output(repo, &["rev-parse", "-q", "--verify", "HEAD"])?.hash(&mut hasher);
    Ok(hasher.finish())
}

// With -q, these commands exit with 1 and no message if HEAD is detached or does not exist yet.
fn optional_output(repo: &Path, args: &[&str]) -> Result<Option<Vec<u8>>, GitError> {
    match git::run(repo, args) {
        Ok(output) => Ok(Some(output)),
        Err(GitError::Failed { code: Some(1), .. }) => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::refs::RefKind;
    use crate::test_support::TempRepo;
    use tempfile::TempDir;

    #[test]
    fn repository_without_commits() {
        let repo = TempRepo::new();
        assert_eq!(load(repo.path()).unwrap().len(), 0);
    }

    #[test]
    fn merge_resolves_parents() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "base");
        repo.git(&["checkout", "-q", "-b", "other"]);
        repo.commit("b.txt", b"2", "on other");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("c.txt", b"3", "on main");
        repo.git(&["merge", "-q", "--no-ff", "-m", "merge", "other"]);

        let store = load(repo.path()).unwrap();

        assert_eq!(store.len(), 4);
        let merge = store.get(0);
        assert_eq!(&*merge.subject, "merge");
        assert_eq!(merge.parents.len(), 2);
        for parent in merge.parents.iter() {
            assert!(*parent > 0 && *parent < store.len());
        }
    }

    #[test]
    fn shallow_clone() {
        let repo = TempRepo::new();
        for name in ["1", "2", "3", "4", "5"] {
            repo.commit("a.txt", name.as_bytes(), name);
        }
        let dir = TempDir::new().unwrap();
        let clone = dir.path().join("clone");
        let url = format!("file://{}", repo.path().display());
        repo.git(&["clone", "-q", "--depth", "2", &url, clone.to_str().unwrap()]);

        let store = load(&clone).unwrap();

        assert_eq!(store.len(), 2);
        assert!(store.get(1).parents.is_empty());
    }

    #[test]
    fn detached_head() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        repo.commit("a.txt", b"2", "two");
        repo.git(&["checkout", "-q", "--detach", "HEAD~1"]);
        repo.commit("b.txt", b"3", "only on head");

        let store = load(repo.path()).unwrap();

        assert_eq!(store.len(), 3);
        assert!((0..3).any(|i| &*store.get(i).subject == "only on head"));
    }

    #[test]
    fn children_before_parents() {
        let repo = TempRepo::new();
        for name in ["1", "2", "3"] {
            repo.commit("a.txt", name.as_bytes(), name);
        }
        let store = load(repo.path()).unwrap();
        for (index, parents) in store.parents().enumerate() {
            assert!(parents.iter().all(|parent| *parent as usize > index));
        }
    }

    // A signed commit and a fake `gpg`: with `log.showSignature` git prepends what the latter
    // prints to the commit record.
    #[test]
    fn signature_in_configuration_does_not_break_loading() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        let tree = repo.git(&["rev-parse", "HEAD^{tree}"]);
        let content = format!(
            "tree {}author Test <test@example.com> 1 +0000\ncommitter Test <test@example.com> 1 +0000\ngpgsig -----BEGIN PGP SIGNATURE-----\n \n abc\n -----END PGP SIGNATURE-----\n\nsigned\n",
            tree
        );
        let object = repo.path().join("signed.txt");
        std::fs::write(&object, content).unwrap();
        let signed = repo.git(&[
            "hash-object",
            "-t",
            "commit",
            "-w",
            object.to_str().unwrap(),
        ]);
        repo.git(&["update-ref", "refs/heads/main", signed.trim()]);
        let fake_gpg = repo.path().join("fake-gpg");
        std::fs::write(
            &fake_gpg,
            "#!/bin/sh\necho 'gpg: fake signature' >&2\nexit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(
            &fake_gpg,
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        repo.git(&["config", "gpg.program", fake_gpg.to_str().unwrap()]);
        repo.git(&["config", "log.showSignature", "true"]);
        assert!(repo
            .git(&["log", "-1", "--format=%s"])
            .contains("gpg: fake signature"));

        let store = load(repo.path()).unwrap();

        assert_eq!(store.len(), 1);
        assert_eq!(&*store.get(0).subject, "signed");
        assert_eq!(&*store.get(0).hash, signed.trim());
    }

    #[test]
    fn loading_does_not_include_the_stash() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        std::fs::write(repo.path().join("a.txt"), "2").unwrap();
        repo.git(&["stash"]);

        let store = load(repo.path()).unwrap();

        assert_eq!(store.len(), 1);
    }

    // `git fast-import` creates the commits at once; a thousand with `git commit` would be slow.
    fn commits_by_the_same_author(repo: &TempRepo, count: u32) {
        let mut stream = String::new();
        for number in 1..=count {
            let message = format!("commit {number}");
            stream += &format!(
                "commit refs/heads/main\nmark :{number}\nauthor Alice <alice@a.com> {number} +0000\ncommitter Alice <alice@a.com> {number} +0000\ndata {}\n{message}\n",
                message.len()
            );
            if number > 1 {
                stream += &format!("from :{}\n", number - 1);
            }
            stream += "\n";
        }
        let mut child = std::process::Command::new("git")
            .current_dir(repo.path())
            .args(["fast-import", "--quiet"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        std::io::Write::write_all(&mut child.stdin.take().unwrap(), stream.as_bytes()).unwrap();
        assert!(child.wait().unwrap().success());
    }

    #[test]
    fn interned_authors_take_one_entry() {
        let repo = TempRepo::new();
        commits_by_the_same_author(&repo, 1000);

        let store = load(repo.path()).unwrap();

        assert_eq!(store.len(), 1000);
        assert_eq!(store.people().len(), 1);
        let person = store.people().get(store.get(0).author);
        assert_eq!((&*person.name, &*person.email), ("Alice", "alice@a.com"));
        assert_eq!(store.get(0).author, store.get(999).committer);
    }

    #[test]
    fn author_and_committer_with_their_dates() {
        let repo = TempRepo::new();
        std::fs::write(repo.path().join("a.txt"), "1").unwrap();
        repo.git(&["add", "-A"]);
        let output = std::process::Command::new("git")
            .current_dir(repo.path())
            .args(["commit", "-q", "-m", "one"])
            .envs([
                ("GIT_AUTHOR_NAME", "Bob"),
                ("GIT_AUTHOR_EMAIL", "bob@b.com"),
                ("GIT_AUTHOR_DATE", "2001-01-01T00:00:00Z"),
                ("GIT_COMMITTER_NAME", "Carol"),
                ("GIT_COMMITTER_EMAIL", "carol@c.com"),
                ("GIT_COMMITTER_DATE", "2002-02-02T00:00:00Z"),
            ])
            .output()
            .unwrap();
        assert!(output.status.success());

        let store = load(repo.path()).unwrap();

        let commit = store.get(0);
        let author = store.people().get(commit.author);
        let committer = store.people().get(commit.committer);
        assert_eq!((&*author.name, &*author.email), ("Bob", "bob@b.com"));
        assert_eq!(
            (&*committer.name, &*committer.email),
            ("Carol", "carol@c.com")
        );
        assert_eq!(commit.author_time, 978307200);
        assert_eq!(commit.commit_time, 1012608000);
        assert_eq!(store.people().len(), 2);
    }

    #[test]
    fn loading_reads_references_with_current_branch_and_tracking() {
        let origin = TempRepo::new();
        origin.commit("a.txt", b"1", "one");
        origin.git(&["tag", "-a", "-m", "annotated", "v1"]);
        let dir = TempDir::new().unwrap();
        let clone = dir.path().join("clone");
        let url = format!("file://{}", origin.path().display());
        origin.git(&["clone", "-q", &url, clone.to_str().unwrap()]);

        let store = load(&clone).unwrap();

        let refs = store.refs();
        let mut found: Vec<_> = refs
            .all
            .iter()
            .map(|git_ref| (&*git_ref.name, git_ref.kind))
            .collect();
        found.sort_by_key(|(name, _)| *name);
        assert_eq!(
            found,
            [
                ("HEAD", RefKind::Head),
                ("main", RefKind::Local),
                ("origin/main", RefKind::Remote),
                ("v1", RefKind::Tag),
            ]
        );
        assert!(refs.all.iter().all(|git_ref| git_ref.commit == 0));
        assert_eq!(&*refs.all[refs.current.unwrap() as usize].name, "main");
        let (local, remote) = refs.tracked.iter().next().map(|(l, r)| (*l, *r)).unwrap();
        assert_eq!(&*refs.all[local as usize].name, "main");
        assert_eq!(&*refs.all[remote as usize].name, "origin/main");
    }

    #[test]
    fn detached_head_has_no_current_branch() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        repo.commit("a.txt", b"2", "two");
        repo.git(&["checkout", "-q", "--detach", "HEAD~1"]);

        let store = load(repo.path()).unwrap();

        assert!(store.refs().current.is_none());
        assert!(store
            .refs()
            .all
            .iter()
            .any(|git_ref| git_ref.kind == RefKind::Head));
    }

    #[test]
    fn fingerprint_changes_when_creating_a_branch() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        let before = refs_fingerprint(repo.path()).unwrap();

        repo.git(&["branch", "new"]);

        assert_ne!(refs_fingerprint(repo.path()).unwrap(), before);
    }

    #[test]
    fn fingerprint_changes_with_detached_head_and_with_a_commit() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        let on_branch = refs_fingerprint(repo.path()).unwrap();
        repo.git(&["checkout", "-q", "--detach"]);
        let detached = refs_fingerprint(repo.path()).unwrap();
        repo.commit("a.txt", b"2", "two");

        assert_ne!(detached, on_branch);
        assert_ne!(refs_fingerprint(repo.path()).unwrap(), detached);
    }

    #[test]
    fn fingerprint_does_not_change_when_editing_a_file() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        let before = refs_fingerprint(repo.path()).unwrap();

        std::fs::write(repo.path().join("a.txt"), "edited").unwrap();
        repo.git(&["stash"]);
        std::fs::write(repo.path().join("a.txt"), "again").unwrap();

        assert_eq!(refs_fingerprint(repo.path()).unwrap(), before);
    }

    #[test]
    fn fingerprint_of_repository_without_commits() {
        let repo = TempRepo::new();
        assert!(refs_fingerprint(repo.path()).is_ok());
    }
}
