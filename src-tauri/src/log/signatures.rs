// The `GPG Signature` column and the signature in the commit details (`GitCommitSignatureLoaderBase`,
// `GitCommitSignatureLoaderSharedCache`). Signatures are read lazily, only for the commits on screen
// or selected: verifying every commit of the log would run gpg thousands of times, so `%G?` never
// goes into the all-commits format (`parse::LOG_FORMAT`).
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::diff;
use crate::git::{self, GitError};
use crate::task::{Runner, TaskId, TaskKind};

// `GitCommitSignatureLoaderBase.loadCommitSignatures`, with `GitHandler`'s `log.showSignature=false`:
// the hashes go on stdin, one per line, and each commit gives four lines.
const ARGS: [&str; 7] = [
    "-c",
    "log.showSignature=false",
    "log",
    "--no-walk=unsorted",
    "--format=%H%n%G?%n%GS%n%GF",
    "--stdin",
    "--",
];
const LINES_PER_COMMIT: usize = 4;
// `SimpleGitCommitSignatureLoader`: a hard-coded title, not in the bundle.
const LOADING: &str = "Loading git commit signatures";
// `GitCommitSignatureLoaderSharedCache`.
const CACHE_SIZE: usize = 1000;
const CACHE_EXPIRY: Duration = Duration::from_secs(60);

// `GitCommitSignature`: `Verified`, the five `NotVerified` reasons, and `Bad`. `N` has no value: an
// unsigned commit gets no entry at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SignatureStatus {
    Verified,
    Unknown,
    Expired,
    ExpiredKey,
    RevokedKey,
    CannotVerify,
    Bad,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Signature {
    pub hash: String,
    pub status: SignatureStatus,
    pub signer: String,
    pub fingerprint: String,
}

// `createSignature`. 241 reports an unknown letter with `LOG.error`; here it only goes to the log
// (Review Focus 3: `P`, for example, shows nothing and says nothing loud).
fn status_of(letter: &str) -> Option<SignatureStatus> {
    match letter {
        "G" => Some(SignatureStatus::Verified),
        "U" => Some(SignatureStatus::Unknown),
        "X" => Some(SignatureStatus::Expired),
        "Y" => Some(SignatureStatus::ExpiredKey),
        "R" => Some(SignatureStatus::RevokedKey),
        "E" => Some(SignatureStatus::CannotVerify),
        "B" => Some(SignatureStatus::Bad),
        "N" => None,
        other => {
            eprintln!("Unknown signature status {other:?}");
            None
        }
    }
}

// `parseSignatures`: groups of four lines. A group whose first line is not a hash is not a commit
// and is skipped.
fn parse(output: &str) -> Vec<Signature> {
    let lines: Vec<&str> = output.lines().collect();
    lines
        .chunks(LINES_PER_COMMIT)
        .filter(|group| group.len() == LINES_PER_COMMIT && diff::check_hash(group[0]).is_ok())
        .filter_map(|group| {
            Some(Signature {
                hash: group[0].to_string(),
                status: status_of(group[1])?,
                signer: group[2].to_string(),
                fingerprint: group[3].to_string(),
            })
        })
        .collect()
}

fn read(
    hashes: &[String],
    run: impl FnOnce(&[&str], &[u8]) -> Result<String, GitError>,
) -> Result<Vec<Signature>, GitError> {
    for hash in hashes {
        diff::check_hash(hash)?;
    }
    if hashes.is_empty() {
        return Ok(Vec::new());
    }
    let input: String = hashes.iter().map(|hash| format!("{hash}\n")).collect();
    Ok(parse(&run(&ARGS, input.as_bytes())?))
}

// The signatures of `hashes`, in the repository's root. Unsigned commits are not in the result.
pub fn signatures(repo: &Path, hashes: &[String]) -> Result<Vec<Signature>, GitError> {
    read(hashes, |args, input| {
        let output = git::run_with_input(repo, args, input)?;
        Ok(String::from_utf8_lossy(&output).into_owned())
    })
}

struct Entry {
    signature: Signature,
    used: Instant,
}

// `GitCommitSignatureLoaderSharedCache`: app-wide, the last `CACHE_SIZE` signatures, each one
// forgotten `CACHE_EXPIRY` after it was last read. Only what was found is kept: an unsigned commit
// is asked again, as in 241.
struct SignatureCache {
    entries: HashMap<(PathBuf, String), Entry>,
    size: usize,
    expiry: Duration,
}

impl SignatureCache {
    fn new(size: usize, expiry: Duration) -> Self {
        Self {
            entries: HashMap::new(),
            size,
            expiry,
        }
    }

    fn get(&mut self, repo: &Path, hash: &str, now: Instant) -> Option<Signature> {
        let key = (repo.to_path_buf(), hash.to_string());
        let entry = self.entries.get_mut(&key)?;
        if now.duration_since(entry.used) >= self.expiry {
            self.entries.remove(&key);
            return None;
        }
        entry.used = now;
        Some(entry.signature.clone())
    }

    fn put(&mut self, repo: &Path, signature: Signature, now: Instant) {
        let key = (repo.to_path_buf(), signature.hash.clone());
        self.entries.insert(
            key,
            Entry {
                signature,
                used: now,
            },
        );
        while self.entries.len() > self.size {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(key) => self.entries.remove(&key),
                None => break,
            };
        }
    }
}

impl Default for SignatureCache {
    fn default() -> Self {
        Self::new(CACHE_SIZE, CACHE_EXPIRY)
    }
}

// `wrapWithCaching`: what is cached comes from there, and git is asked only for the rest.
fn cached_or_read(
    cache: &Mutex<SignatureCache>,
    repo: &Path,
    hashes: &[String],
    run: impl FnOnce(&[&str], &[u8]) -> Result<String, GitError>,
) -> Result<Vec<Signature>, GitError> {
    let lock = || cache.lock().unwrap_or_else(PoisonError::into_inner);
    let (mut found, missing) = {
        let mut cache = lock();
        let now = Instant::now();
        let mut found = Vec::new();
        let mut missing = Vec::new();
        for hash in hashes {
            match cache.get(repo, hash, now) {
                Some(signature) => found.push(signature),
                None => missing.push(hash.clone()),
            }
        }
        (found, missing)
    };
    let loaded = read(&missing, run)?;
    let mut cache = lock();
    let now = Instant::now();
    for signature in &loaded {
        cache.put(repo, signature.clone(), now);
    }
    found.extend(loaded);
    Ok(found)
}

// Who asks: each table and each details panel has its own loader in 241
// (`VcsLogExternalStatusColumnService`, `VcsLogCommitSelectionListenerForDetails`), and a new request
// of one cancels only its own previous one (`GitCommitSignatureLoaderBase.currentIndicator`).
// A requester is numbered inside its project window, so `pending` is keyed by the owner too.
#[derive(Default)]
pub struct SignatureLoads {
    cache: Arc<Mutex<SignatureCache>>,
    pending: Mutex<HashMap<(String, u32), TaskId>>,
}

impl SignatureLoads {
    // A read task that only runs `git log`: it changes nothing, so nothing is saved first. With
    // `pending` locked from cancelling until the new task is recorded.
    pub fn start(
        &self,
        runner: &Runner,
        repo: &Path,
        owner: &str,
        requester: u32,
        hashes: Vec<String>,
    ) -> Result<TaskId, GitError> {
        for hash in &hashes {
            diff::check_hash(hash)?;
        }
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        let key = (owner.to_string(), requester);
        if let Some(previous) = pending.get(&key) {
            runner.cancel(owner, *previous);
        }
        let cache = self.cache.clone();
        let path = repo.to_path_buf();
        let id = runner.start(
            owner,
            repo,
            TaskKind::Read,
            LOADING.to_string(),
            move |context| {
                cached_or_read(&cache, &path, &hashes, |args, input| {
                    Ok(context.git_with_input(args, input)?.stdout)
                })
            },
        );
        pending.insert(key, id);
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::RepoLocks;
    use crate::console::Console;
    use crate::log::parse::LOG_FORMAT;
    use crate::task::{TaskEvent, TaskState};
    use crate::test_support::TempRepo;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use std::sync::mpsc::{self, Receiver};
    use std::thread;
    use tempfile::TempDir;

    fn head(repo: &TempRepo) -> String {
        repo.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    fn public_key(path: &Path) -> String {
        let with = |extension: &str| path.with_extension(extension);
        let key = with("pub");
        if !key.exists() {
            let status = Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                .arg(path)
                .status()
                .unwrap();
            assert!(status.success());
        }
        fs::read_to_string(key).unwrap().trim().to_string()
    }

    // `principal keytype base64`, in the repository's own config only.
    fn allow(repo: &TempRepo, principal: &str, key: &str) {
        let file = repo.path().join(".git/allowed-signers");
        let mut parts = key.split_whitespace();
        let (kind, data) = (parts.next().unwrap(), parts.next().unwrap());
        fs::write(&file, format!("{principal} {kind} {data}\n")).unwrap();
        repo.git(&[
            "config",
            "--local",
            "gpg.ssh.allowedSignersFile",
            &file.to_string_lossy(),
        ]);
    }

    fn fingerprint(key: &str) -> String {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("key.pub");
        fs::write(&file, key).unwrap();
        let output = Command::new("ssh-keygen")
            .args(["-l", "-E", "sha256", "-f"])
            .arg(&file)
            .output()
            .unwrap();
        let text = String::from_utf8(output.stdout).unwrap();
        text.split_whitespace().nth(1).unwrap().to_string()
    }

    // P§1.7: no allowed-signers file is `N`; the signer in it is `G` with the principal and the
    // `SHA256:` fingerprint; a key that is not in it is `U`.
    #[test]
    fn ssh_signed_commits_read_as_241_with_and_without_allowed_signers() {
        let repo = TempRepo::new();
        repo.sign_commits();
        repo.commit("a.txt", b"1\n", "signed");
        let hash = head(&repo);
        let hashes = vec![hash.clone()];

        assert_eq!(signatures(repo.path(), &hashes).unwrap(), []);

        let own = public_key(&repo.path().join(".git/test-signing-key"));
        allow(&repo, "test@example.com", &own);
        assert_eq!(
            signatures(repo.path(), &hashes).unwrap(),
            [Signature {
                hash: hash.clone(),
                status: SignatureStatus::Verified,
                signer: "test@example.com".to_string(),
                fingerprint: fingerprint(&own),
            }]
        );

        let other = public_key(&repo.path().join(".git/other-key"));
        allow(&repo, "someone@example.com", &other);
        let unknown = signatures(repo.path(), &hashes).unwrap();
        assert_eq!(unknown.len(), 1);
        assert_eq!(unknown[0].status, SignatureStatus::Unknown);
        assert_eq!(unknown[0].signer, "");
        assert_eq!(unknown[0].fingerprint, fingerprint(&own));
    }

    #[test]
    fn unsigned_and_unknown_letters_give_no_entry() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "unsigned");
        assert_eq!(signatures(repo.path(), &[head(&repo)]).unwrap(), []);

        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let c = "c".repeat(40);
        let output = format!("{a}\nN\n\n\n{b}\nP\nsomeone\nSHA256:x\n{c}\nB\n\n\n");
        assert_eq!(
            parse(&output),
            [Signature {
                hash: c.clone(),
                status: SignatureStatus::Bad,
                signer: String::new(),
                fingerprint: String::new(),
            }]
        );

        let letters = [
            ("G", SignatureStatus::Verified),
            ("U", SignatureStatus::Unknown),
            ("X", SignatureStatus::Expired),
            ("Y", SignatureStatus::ExpiredKey),
            ("R", SignatureStatus::RevokedKey),
            ("E", SignatureStatus::CannotVerify),
            ("B", SignatureStatus::Bad),
        ];
        for (letter, status) in letters {
            assert_eq!(status_of(letter), Some(status), "{letter}");
        }
        assert_eq!(status_of("N"), None);
        assert_eq!(status_of(""), None);
    }

    // Hashes reach git only on stdin, and only once checked.
    #[test]
    fn hashes_are_checked_before_git_runs() {
        let mut ran = false;
        let refused = read(&["--all".to_string()], |_, _| {
            ran = true;
            Ok(String::new())
        });
        assert!(refused.is_err());
        assert!(!ran);

        let mut seen = (Vec::new(), Vec::new());
        read(&["d".repeat(40)], |args, input| {
            seen = (
                args.iter().map(|arg| arg.to_string()).collect(),
                input.to_vec(),
            );
            Ok(String::new())
        })
        .unwrap();
        assert_eq!(
            seen.0.join(" "),
            "-c log.showSignature=false log --no-walk=unsorted --format=%H%n%G?%n%GS%n%GF --stdin --"
        );
        assert_eq!(seen.1, format!("{}\n", "d".repeat(40)).into_bytes());
    }

    // Verifying every commit of the log would run gpg once per signed commit: the all-commits
    // format never asks for a signature.
    #[test]
    fn the_all_commits_log_format_has_no_signature_placeholder() {
        assert!(!LOG_FORMAT.contains("%G"), "{LOG_FORMAT}");
        let store = include_str!("store.rs");
        let store = &store[..store.find("#[cfg(test)]").unwrap()];
        assert!(!store.contains("%G"));
    }

    fn signature(hash: &str) -> Signature {
        Signature {
            hash: hash.to_string(),
            status: SignatureStatus::Verified,
            signer: "someone".to_string(),
            fingerprint: "SHA256:x".to_string(),
        }
    }

    #[test]
    fn the_cache_keeps_the_last_ones_and_forgets_them_a_minute_after_their_last_read() {
        let repo = Path::new("/repo");
        let mut cache = SignatureCache::new(2, Duration::from_secs(60));
        let start = Instant::now();
        let at = |seconds| start + Duration::from_secs(seconds);
        cache.put(repo, signature("a"), at(0));
        cache.put(repo, signature("b"), at(1));
        assert!(cache.get(repo, "a", at(2)).is_some());
        // `b` is the least recently used one now.
        cache.put(repo, signature("c"), at(3));
        assert!(cache.get(repo, "b", at(4)).is_none());
        assert!(cache.get(Path::new("/other"), "a", at(4)).is_none());

        // Reading it moves its expiry.
        assert!(cache.get(repo, "a", at(50)).is_some());
        assert!(cache.get(repo, "a", at(109)).is_some());
        assert!(cache.get(repo, "c", at(109)).is_none());
        assert!(cache.get(repo, "a", at(169)).is_none());
    }

    #[test]
    fn what_is_cached_is_not_asked_again() {
        let cache = Mutex::new(SignatureCache::default());
        let repo = Path::new("/repo");
        let (a, b) = ("a".repeat(40), "b".repeat(40));
        let output = format!("{a}\nG\nsomeone\nSHA256:x\n");
        let first = cached_or_read(&cache, repo, &[a.clone(), b.clone()], |_, _| Ok(output));
        assert_eq!(first.unwrap(), [signature(&a)]);

        let mut asked = String::new();
        let second = cached_or_read(&cache, repo, &[a.clone(), b.clone()], |_, input| {
            asked = String::from_utf8(input.to_vec()).unwrap();
            Ok(String::new())
        });
        assert_eq!(second.unwrap(), [signature(&a)]);
        // The unsigned one is asked again; the signed one comes from the cache.
        assert_eq!(asked, format!("{b}\n"));
    }

    struct Harness {
        runner: Runner,
        events: Receiver<TaskEvent>,
        seen: Vec<TaskEvent>,
        dir: TempDir,
    }

    impl Harness {
        // A fake git that marks its start and waits for `release`.
        fn new() -> Self {
            let dir = TempDir::new().unwrap();
            let script = dir.path().join("git");
            let body = format!(
                "#!/bin/sh\ntouch '{0}/started.'$$\nwhile [ ! -e '{0}/release' ]; do sleep 0.01; done\n",
                dir.path().display()
            );
            fs::write(&script, body).unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
            let runner = Runner::with(
                Arc::new(RepoLocks::default()),
                Arc::new(Console::default()),
                Some(script),
                Duration::from_secs(5),
            );
            let (send, events) = mpsc::channel();
            let send = Mutex::new(send);
            runner.listen(move |event| {
                let _ = send.lock().unwrap().send(event.clone());
            });
            Self {
                runner,
                events,
                seen: Vec::new(),
                dir,
            }
        }

        fn wait_started(&self, count: usize) {
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                let started = fs::read_dir(self.dir.path())
                    .unwrap()
                    .filter(|entry| {
                        let name = entry.as_ref().unwrap().file_name();
                        name.to_string_lossy().starts_with("started.")
                    })
                    .count();
                if started >= count {
                    return;
                }
                assert!(Instant::now() < deadline, "the fake did not start");
                thread::sleep(Duration::from_millis(10));
            }
        }

        fn finished(&mut self, id: TaskId) -> TaskEvent {
            loop {
                if let Some(position) = self.seen.iter().position(|event| event.id == id) {
                    return self.seen.remove(position);
                }
                let event = self.events.recv_timeout(Duration::from_secs(20)).unwrap();
                if event.state != TaskState::Running {
                    self.seen.push(event);
                }
            }
        }
    }

    // Review Focus 3: a new request of a table cancels its previous one, and only its own.
    #[test]
    fn a_new_request_cancels_the_previous_one_of_the_same_requester_only() {
        let mut harness = Harness::new();
        let loads = SignatureLoads::default();
        let repo = TempDir::new().unwrap();
        let hashes = || vec!["e".repeat(40)];
        let start = |requester, hashes| {
            loads
                .start(&harness.runner, repo.path(), "main", requester, hashes)
                .unwrap()
        };

        let table = start(1, hashes());
        let details = start(2, hashes());
        harness.wait_started(2);
        let again = start(1, Vec::new());

        assert_eq!(harness.finished(table).state, TaskState::Cancelled);
        assert_eq!(harness.finished(again).state, TaskState::Done);
        fs::write(harness.dir.path().join("release"), "").unwrap();
        let other = harness.finished(details);
        assert_eq!(other.state, TaskState::Done);
        assert_eq!(other.value, Some(serde_json::json!([])));

        let refused = loads.start(
            &harness.runner,
            repo.path(),
            "main",
            1,
            vec!["HEAD".to_string()],
        );
        assert!(refused.is_err());
    }

    // Requesters are numbered inside each project window: the same number in another window is
    // another table.
    #[test]
    fn the_same_requester_of_another_window_is_not_cancelled() {
        let mut harness = Harness::new();
        let loads = SignatureLoads::default();
        let repo = TempDir::new().unwrap();
        let hashes = || vec!["e".repeat(40)];
        let start = |owner, hashes| {
            loads
                .start(&harness.runner, repo.path(), owner, 1, hashes)
                .unwrap()
        };

        let first = start("main", hashes());
        let second = start("main-2", hashes());
        harness.wait_started(2);
        let again = start("main", Vec::new());

        assert_eq!(harness.finished(first).state, TaskState::Cancelled);
        assert_eq!(harness.finished(again).state, TaskState::Done);
        fs::write(harness.dir.path().join("release"), "").unwrap();
        assert_eq!(harness.finished(second).state, TaskState::Done);
    }
}
