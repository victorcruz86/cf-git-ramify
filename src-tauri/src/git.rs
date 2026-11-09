use std::env;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{OnceLock, PoisonError, RwLock};
use std::thread;
use std::time::Duration;

const FALLBACK_BINARIES: [&str; 2] = ["/opt/homebrew/bin/git", "/usr/bin/git"];
const READ_BUFFER_BYTES: usize = 64 * 1024;
const LOCK_RETRIES: u32 = 5;
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(200);
pub const PATHS_PER_COMMAND: usize = 200;
// With `core.precomposeunicode`, git converts arguments to NFC and an NFD index entry would not
// be found. Commands that look things up in the index must see their exact bytes.
const EXACT_NAMES: [&str; 2] = ["-c", "core.precomposeunicode=false"];
const VERSION_PREFIX: &str = "git version ";

// The settings path. It is read on every call so a change takes effect without a restart.
static CONFIGURED: RwLock<Option<PathBuf>> = RwLock::new(None);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitError {
    NotInstalled,
    NotARepository {
        path: String,
    },
    Failed {
        args: Vec<String>,
        code: Option<i32>,
        stdout: String,
        stderr: String,
    },
    Io(String),
    IndexLocked,
    // A file changed on disk after the dialog read it: nothing was written.
    Outdated,
    // The user cancelled a long task; it is not a git failure.
    Cancelled,
}

pub fn run(repo: &Path, args: &[&str]) -> Result<Vec<u8>, GitError> {
    run_with_binary(&git_binary(), repo, args)
}

// With extra environment variables (for example, `GIT_INDEX_FILE` to work on a temporary index
// without touching the real one).
pub fn run_with_env(
    repo: &Path,
    args: &[&str],
    envs: &[(&str, &Path)],
) -> Result<Vec<u8>, GitError> {
    let binary = git_binary();
    let output = command(&binary, repo, args)?
        .envs(envs.iter().copied())
        .output()
        .map_err(|error| spawn_error(&binary, error))?;
    check_status(repo, args, output.status, &output.stdout, &output.stderr)?;
    Ok(output.stdout)
}

// The input is written on another thread so we do not block if git fills its output first. If git
// ends without reading all of it, the write failure says nothing: its exit code decides.
pub fn run_with_input(repo: &Path, args: &[&str], input: &[u8]) -> Result<Vec<u8>, GitError> {
    let binary = git_binary();
    let mut child = command(&binary, repo, args)?
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| spawn_error(&binary, error))?;
    let mut stdin = child.stdin.take().expect("stdin with pipe");
    let output = thread::scope(|scope| {
        scope.spawn(move || {
            let _ = stdin.write_all(input);
        });
        child.wait_with_output()
    })
    .map_err(io_error)?;
    check_status(repo, args, output.status, &output.stdout, &output.stderr)?;
    Ok(output.stdout)
}

// Another Git process (or the IDE itself) may hold the index for a moment: wait and retry.
pub fn run_mutating(repo: &Path, args: &[&str]) -> Result<Vec<u8>, GitError> {
    retrying(|| run(repo, args))
}

// `lock` is passed as git's input. The child thus inherits the same file description, and with it
// the `flock`, and keeps it until it exits even if this process dies. Git does not pass it to its
// hooks, but it stays alive while they run.
pub fn run_mutating_holding(repo: &Path, args: &[&str], lock: &File) -> Result<Vec<u8>, GitError> {
    retrying(|| {
        let stdin = lock.try_clone().map_err(io_error)?;
        let binary = git_binary();
        let output = command(&binary, repo, args)?
            .stdin(Stdio::from(stdin))
            .output()
            .map_err(|error| spawn_error(&binary, error))?;
        check_status(repo, args, output.status, &output.stdout, &output.stderr)?;
        Ok(output.stdout)
    })
}

pub(crate) fn retrying<T>(mut attempt: impl FnMut() -> Result<T, GitError>) -> Result<T, GitError> {
    for _ in 0..LOCK_RETRIES {
        match attempt() {
            Err(error) if is_index_locked(&error) => thread::sleep(LOCK_RETRY_DELAY),
            result => return result,
        }
    }
    match attempt() {
        Err(error) if is_index_locked(&error) => Err(GitError::IndexLocked),
        result => result,
    }
}

// Only git's own message counts (with `LC_ALL=C`): `update-index` repeats the rejected path in its
// errors, and that one may be named `index.lock`.
fn is_index_locked(error: &GitError) -> bool {
    matches!(error, GitError::Failed { stderr, .. } if stderr.lines().any(|line| {
        line.starts_with("fatal: Unable to create '") && line.contains("index.lock': File exists.")
    }))
}

pub fn run_with_binary(binary: &Path, repo: &Path, args: &[&str]) -> Result<Vec<u8>, GitError> {
    let output = command(binary, repo, args)?
        .output()
        .map_err(|error| spawn_error(binary, error))?;
    check_status(repo, args, output.status, &output.stdout, &output.stderr)?;
    Ok(output.stdout)
}

pub fn exact<'a>(args: &[&'a str]) -> Vec<&'a str> {
    EXACT_NAMES.iter().chain(args).copied().collect()
}

// Runs git with the paths as literals (never as patterns) and in batches, so as not to exceed
// the system argument limit.
pub fn git_with_paths(
    repo: &Path,
    mutating: bool,
    head: &[&str],
    paths: &[String],
) -> Result<Vec<u8>, GitError> {
    let mut output = Vec::new();
    for chunk in paths.chunks(PATHS_PER_COMMAND) {
        let mut args = vec!["--literal-pathspecs"];
        args.extend_from_slice(head);
        args.push("--");
        args.extend(chunk.iter().map(String::as_str));
        output.extend(if mutating {
            run_mutating(repo, &args)?
        } else {
            run(repo, &args)?
        });
    }
    Ok(output)
}

// A branch with no commits is the only legitimate case without HEAD; any other failure is propagated.
pub fn head_exists(repo: &Path) -> Result<bool, GitError> {
    match run(repo, &["rev-parse", "-q", "--verify", "HEAD"]) {
        Ok(_) => Ok(true),
        Err(GitError::Failed { code: Some(1), .. }) => {
            if is_unborn(repo)? {
                Ok(false)
            } else {
                Err(GitError::Io(
                    "Cannot read the last commit: the current branch seems to be damaged"
                        .to_string(),
                ))
            }
        }
        Err(error) => Err(error),
    }
}

pub fn git_path(repo: &Path, name: &str) -> Result<PathBuf, GitError> {
    let output = run(repo, &["rev-parse", "--git-path", name])?;
    let path = String::from_utf8(output)
        .map_err(|_| GitError::Io("Invalid output from git rev-parse".to_string()))?;
    Ok(repo.join(path.trim_end_matches('\n')))
}

// Direct target of HEAD, only if it is exactly `ref: refs/heads/<branch>` and an optional final
// line. `symbolic-ref` is not used: it follows the chain, and a branch with `ref: <something>`
// inside exists even if that target does not (`--no-recurse` is not in git 2.38).
fn head_target(repo: &Path) -> Result<Option<String>, GitError> {
    let Ok(contents) = fs::read_to_string(git_path(repo, "HEAD")?) else {
        return Ok(None);
    };
    let target = contents
        .strip_prefix("ref: ")
        .map(|target| target.strip_suffix('\n').unwrap_or(target))
        .filter(|target| target.starts_with("refs/heads/"));
    let Some(target) = target else {
        return Ok(None);
    };
    match run(repo, &["check-ref-format", target]) {
        Ok(_) => Ok(Some(target.to_string())),
        Err(GitError::Failed { .. }) => Ok(None),
        Err(error) => Err(error),
    }
}

// `rev-parse` also exits with 1 if the branch exists and is damaged. It is only unborn if HEAD
// points directly at a branch that does not exist in any form, neither loose nor packed.
fn is_unborn(repo: &Path) -> Result<bool, GitError> {
    let Some(target) = head_target(repo)? else {
        return Ok(false);
    };
    let loose = git_path(repo, &target)?;
    match loose.symlink_metadata() {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        _ => return Ok(false),
    }
    match fs::read(git_path(repo, "packed-refs")?) {
        Ok(packed) => Ok(!String::from_utf8_lossy(&packed)
            .lines()
            .any(|line| line.split_once(' ').is_some_and(|(_, name)| name == target))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(_) => Ok(false),
    }
}

pub fn stream(
    repo: &Path,
    args: &[&str],
    delimiter: u8,
    on_record: impl FnMut(&[u8]),
) -> Result<(), GitError> {
    stream_with_binary(&git_binary(), repo, args, delimiter, on_record)
}

pub fn stream_with_binary(
    binary: &Path,
    repo: &Path,
    args: &[&str],
    delimiter: u8,
    mut on_record: impl FnMut(&[u8]),
) -> Result<(), GitError> {
    let mut child = command(binary, repo, args)?
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| spawn_error(binary, error))?;
    let mut stdout = child.stdout.take().expect("stdout with pipe");
    let mut stderr = child.stderr.take().expect("stderr with pipe");
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).map(|_| bytes)
    });

    let mut buffer = vec![0; READ_BUFFER_BYTES];
    let mut pending = Vec::new();
    loop {
        let read = stdout.read(&mut buffer).map_err(io_error)?;
        if read == 0 {
            break;
        }
        let mut chunk = &buffer[..read];
        while let Some(end) = chunk.iter().position(|byte| *byte == delimiter) {
            if pending.is_empty() {
                on_record(&chunk[..end]);
            } else {
                pending.extend_from_slice(&chunk[..end]);
                on_record(&pending);
                pending.clear();
            }
            chunk = &chunk[end + 1..];
        }
        pending.extend_from_slice(chunk);
    }
    if !pending.is_empty() {
        on_record(&pending);
    }

    let status = child.wait().map_err(io_error)?;
    let stderr = stderr_reader
        .join()
        .expect("the stderr thread must not fail")
        .map_err(io_error)?;
    check_status(repo, args, status, &[], &stderr)
}

pub(crate) fn command(binary: &Path, repo: &Path, args: &[&str]) -> Result<Command, GitError> {
    if !repo.is_dir() {
        return Err(not_a_repository(repo));
    }
    let mut command = Command::new(binary);
    command
        .current_dir(repo)
        .args(args)
        .env("LC_ALL", "C")
        .env("GIT_OPTIONAL_LOCKS", "0")
        // `GitCommand.IJ_HANDLER_MARKER_ENV`: lets the user's hooks tell that the IDE runs git.
        .env("INTELLIJ_GIT_EXECUTABLE", "true");
    // The tests' git does not read the machine's configuration; the app's does.
    #[cfg(test)]
    crate::test_support::isolate(&mut command);
    Ok(command)
}

pub(crate) fn check_status(
    repo: &Path,
    args: &[&str],
    status: ExitStatus,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<(), GitError> {
    if status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(stderr).into_owned();
    if stderr.contains("not a git repository") {
        return Err(not_a_repository(repo));
    }
    Err(GitError::Failed {
        args: args.iter().map(|arg| arg.to_string()).collect(),
        code: status.code(),
        stdout: String::from_utf8_lossy(stdout).into_owned(),
        stderr,
    })
}

fn not_a_repository(repo: &Path) -> GitError {
    GitError::NotARepository {
        path: repo.display().to_string(),
    }
}

// A folder or a file without execute permission exist, but cannot be launched.
pub(crate) fn spawn_error(binary: &Path, error: io::Error) -> GitError {
    if error.kind() == io::ErrorKind::NotFound {
        GitError::NotInstalled
    } else {
        GitError::Io(format!("Could not run \"{}\": {error}", binary.display()))
    }
}

pub(crate) fn io_error(error: io::Error) -> GitError {
    GitError::Io(error.to_string())
}

pub fn set_binary(path: Option<&str>) {
    *CONFIGURED.write().unwrap_or_else(PoisonError::into_inner) = path.map(PathBuf::from);
}

pub fn configured_binary() -> Option<PathBuf> {
    CONFIGURED
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

fn detected_binary() -> &'static Path {
    static BINARY: OnceLock<PathBuf> = OnceLock::new();
    BINARY.get_or_init(find_binary)
}

pub(crate) fn git_binary() -> PathBuf {
    configured_binary().unwrap_or_else(|| detected_binary().to_path_buf())
}

// The version of a path not yet saved, or of the detected one. Git does not need a repository here.
pub fn version_of(path: Option<&str>) -> Result<String, GitError> {
    let binary = path.map_or_else(|| detected_binary().to_path_buf(), PathBuf::from);
    let output = run_with_binary(&binary, &env::temp_dir(), &["--version"])?;
    let output = String::from_utf8_lossy(&output);
    match output.trim().strip_prefix(VERSION_PREFIX) {
        Some(version) => Ok(version.to_string()),
        None => Err(GitError::Io(unidentified_version(&binary))),
    }
}

fn unidentified_version(binary: &Path) -> String {
    format!("Cannot identify version of Git executable {}", binary.display())
}

// Launched from Finder, the app does not inherit the terminal's PATH.
fn find_binary() -> PathBuf {
    env::var_os("PATH")
        .into_iter()
        .flat_map(|path| env::split_paths(&path).collect::<Vec<_>>())
        .map(|directory| directory.join("git"))
        .chain(FALLBACK_BINARIES.iter().map(PathBuf::from))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| PathBuf::from("git"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_unidentifiable_git_names_git_with_a_capital() {
        let tools = TempDir::new().unwrap();
        let other = tools.path().join("another");
        fs::write(&other, "#!/bin/sh\necho hello\n").unwrap();
        fs::set_permissions(&other, fs::Permissions::from_mode(0o755)).unwrap();
        let Err(GitError::Io(message)) = version_of(other.to_str()) else {
            panic!("the version should not be identified");
        };
        assert_eq!(message, format!("Cannot identify version of Git executable {}", other.display()));
    }

    use super::*;
    use crate::test_support::{TempRepo, DAMAGED_REFS};
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    #[test]
    fn every_git_command_carries_intellij_git_executable() {
        let repo = TempRepo::new();
        let output = run(repo.path(), &["-c", "alias.x=!echo $INTELLIJ_GIT_EXECUTABLE", "x"]).unwrap();
        assert_eq!(output, b"true\n");
    }

    #[test]
    fn run_returns_output() {
        let repo = TempRepo::new();
        let output = run(repo.path(), &["rev-parse", "--is-inside-work-tree"]).unwrap();
        assert_eq!(output, b"true\n");
    }

    #[test]
    fn folder_without_repository() {
        let dir = TempDir::new().unwrap();
        let error = run(dir.path(), &["status"]).unwrap_err();
        assert_eq!(
            error,
            GitError::NotARepository {
                path: dir.path().display().to_string()
            }
        );
    }

    #[test]
    fn missing_git_binary() {
        let repo = TempRepo::new();
        let error = run_with_binary(Path::new("/does/not/exist/git"), repo.path(), &["status"])
            .unwrap_err();
        assert_eq!(error, GitError::NotInstalled);
    }

    #[test]
    fn failed_command() {
        let repo = TempRepo::new();
        match run(repo.path(), &["log", "--nonexistent-branch"]).unwrap_err() {
            GitError::Failed { stderr, args, .. } => {
                assert!(!stderr.is_empty());
                assert_eq!(args, ["log", "--nonexistent-branch"]);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn failure_keeps_stdout_and_stderr() {
        let repo = TempRepo::new();
        let script = repo.path().join("fails.sh");
        std::fs::write(&script, "#!/bin/sh\necho output\necho error >&2\nexit 3\n").unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();

        match run_with_binary(&script, repo.path(), &[]).unwrap_err() {
            GitError::Failed {
                stdout,
                stderr,
                code,
                ..
            } => {
                assert_eq!(stdout, "output\n");
                assert_eq!(stderr, "error\n");
                assert_eq!(code, Some(3));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    // The tests read no configuration of the machine (`test_support::isolate`), neither through
    // the code under test nor through the git they spawn themselves: only the repository's.
    #[test]
    fn the_tests_git_reads_only_the_repository_configuration() {
        let repo = TempRepo::new();
        let scopes = |output: &[u8]| -> Vec<String> {
            let mut scopes: Vec<String> = String::from_utf8_lossy(output)
                .lines()
                .filter_map(|line| line.split('\t').next())
                .map(str::to_string)
                .collect();
            scopes.dedup();
            scopes
        };
        let args = ["config", "--list", "--show-scope"];

        let through_run = run(repo.path(), &args).unwrap();
        let spawned = crate::test_support::git_command()
            .current_dir(repo.path())
            .args(args)
            .output()
            .unwrap();

        assert_eq!(scopes(&through_run), ["local"]);
        assert_eq!(scopes(&spawned.stdout), ["local"]);
    }

    // Paths that the user's own `~/.config/git/ignore` (git's default `core.excludesFile`) would
    // ignore, plus a few common ones.
    fn paths_the_user_would_ignore() -> Vec<String> {
        let mut paths: Vec<String> = [".DS_Store", ".claude/x", ".idea/x", "x.log", "x.swp"]
            .map(str::to_string)
            .to_vec();
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let file = home.map(|home| home.join(".config/git/ignore"));
        let text = file
            .and_then(|file| fs::read_to_string(file).ok())
            .unwrap_or_default();
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with(['#', '!']) || line.contains(['[', '\\']) {
                continue;
            }
            let folder = line.ends_with('/');
            let name = line.trim_matches('/').replace(['*', '?'], "x");
            paths.push(if folder { format!("{name}/x") } else { name });
        }
        paths
    }

    // Git also reads `$XDG_CONFIG_HOME/git/ignore` and `attributes` (by default under `~/.config`)
    // without any setting: the tests point `XDG_CONFIG_HOME` at an empty folder of their own, and
    // no path is ignored or given an attribute by a file outside the repository.
    #[test]
    fn the_tests_git_reads_no_ignore_or_attributes_file_of_the_user() {
        let repo = TempRepo::new();
        let (_, xdg) = crate::test_support::isolated_places();
        let command = crate::test_support::git_command();
        let set = command
            .get_envs()
            .find(|(name, _)| *name == "XDG_CONFIG_HOME")
            .and_then(|(_, value)| value);
        assert_eq!(set, Some(xdg.as_os_str()));
        assert_eq!(fs::read_dir(xdg).unwrap().count(), 0);
        let input: String = paths_the_user_would_ignore()
            .iter()
            .map(|path| format!("{path}\n"))
            .collect();

        // `check-ignore` exits with 1 when nothing is ignored.
        let mut check = crate::test_support::git_command();
        let mut child = check
            .current_dir(repo.path())
            .args(["check-ignore", "--no-index", "-v", "-n", "--stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let ignored = child.wait_with_output().unwrap();
        let attributes = run_with_input(
            repo.path(),
            &["check-attr", "-a", "--stdin"],
            input.as_bytes(),
        )
        .unwrap();

        let ignored = String::from_utf8_lossy(&ignored.stdout);
        let matched: Vec<&str> = ignored
            .lines()
            .filter(|line| !line.starts_with("::"))
            .collect();
        assert!(matched.is_empty(), "{matched:?}");
        assert_eq!(String::from_utf8_lossy(&attributes), "");
    }

    #[test]
    fn run_with_input_passes_the_input_and_returns_the_output() {
        let repo = TempRepo::new();
        let big = vec![b'x'; 2_000_000];

        let hash = run_with_input(repo.path(), &["hash-object", "-w", "--stdin"], &big).unwrap();

        let hash = String::from_utf8(hash).unwrap();
        let stored = run(repo.path(), &["cat-file", "blob", hash.trim()]).unwrap();
        assert_eq!(stored, big);
    }

    #[test]
    fn run_with_input_propagates_the_failure() {
        let repo = TempRepo::new();

        let error = run_with_input(repo.path(), &["not-a-command"], b"x").unwrap_err();

        assert!(matches!(error, GitError::Failed { .. }));
    }

    #[test]
    fn stream_splits_records() {
        let repo = TempRepo::new();
        for name in ["one", "two", "three"] {
            repo.commit("a.txt", name.as_bytes(), name);
        }
        let mut records = Vec::new();
        stream(repo.path(), &["log", "-z", "--format=%s"], 0, |record| {
            records.push(String::from_utf8_lossy(record).into_owned())
        })
        .unwrap();
        assert_eq!(records, ["three", "two", "one"]);
    }

    #[test]
    fn stream_with_large_stderr_output_does_not_block() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"x", "one");
        let script = repo.path().join("noisy.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\nhead -c 1000000 /dev/zero | tr '\\0' 'e' >&2\nprintf 'hello\\0'\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let mut records = Vec::new();
        stream_with_binary(&script, repo.path(), &[], 0, |record| {
            records.push(record.to_vec())
        })
        .unwrap();
        assert_eq!(records, [b"hello".to_vec()]);
    }

    #[test]
    fn run_mutating_waits_for_the_lock() {
        let repo = TempRepo::new();
        std::fs::write(repo.path().join("a.txt"), b"1").unwrap();
        let lock = repo.path().join(".git/index.lock");
        std::fs::write(&lock, b"").unwrap();
        let release = thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            std::fs::remove_file(lock).unwrap();
        });

        let result = run_mutating(repo.path(), &["add", "--", "a.txt"]);

        release.join().unwrap();
        result.unwrap();
        assert_eq!(repo.git(&["status", "--porcelain"]), "A  a.txt\n");
    }

    #[test]
    fn run_mutating_reports_if_still_locked() {
        let repo = TempRepo::new();
        std::fs::write(repo.path().join("a.txt"), b"1").unwrap();
        std::fs::write(repo.path().join(".git/index.lock"), b"").unwrap();

        let error = run_mutating(repo.path(), &["add", "--", "a.txt"]).unwrap_err();

        assert_eq!(error, GitError::IndexLocked);
    }

    fn failed(stderr: &str) -> GitError {
        GitError::Failed {
            args: Vec::new(),
            code: Some(128),
            stdout: String::new(),
            stderr: stderr.to_string(),
        }
    }

    // `update-index` repeats the rejected path: a name with `index.lock` is not a lock.
    #[test]
    fn path_with_index_lock_is_not_a_lock() {
        let rejected = failed(
            "error: 'tools/index.lock' appears as both a file and as a directory\n\
             fatal: git update-index: --cacheinfo cannot add tools/index.lock\n",
        );
        assert!(!is_index_locked(&rejected));

        let locked = failed(
            "fatal: Unable to create '/private/var/x/.git/index.lock': File exists.\n\n\
             Another git process seems to be running in this repository.\n",
        );
        assert!(is_index_locked(&locked));
    }

    #[test]
    fn run_mutating_does_not_retry_other_errors() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        fs::write(repo.path().join("a.txt"), "2").unwrap();
        repo.git(&["add", "a.txt"]);
        let hook = repo.path().join(".git/hooks/pre-commit");
        fs::write(&hook, "#!/bin/sh\necho attempt >> .git/attempts\nexit 1\n").unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();

        let error = run_mutating(repo.path(), &["commit", "-m", "two"]).unwrap_err();

        assert!(matches!(error, GitError::Failed { .. }));
        let attempts = fs::read_to_string(repo.path().join(".git/attempts")).unwrap();
        assert_eq!(attempts.lines().count(), 1);
    }

    #[test]
    fn branch_without_commits_has_no_head() {
        let repo = TempRepo::new();
        assert_eq!(head_exists(repo.path()), Ok(false));

        repo.git(&["pack-refs", "--all"]);
        repo.git(&["symbolic-ref", "HEAD", "refs/heads/other/branch"]);
        assert_eq!(head_exists(repo.path()), Ok(false));
    }

    #[test]
    fn branch_without_commits_next_to_packed_ones_has_no_head() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        repo.git(&["pack-refs", "--all"]);
        repo.git(&["checkout", "-q", "--orphan", "new"]);

        assert_eq!(head_exists(repo.path()), Ok(false));
    }

    #[test]
    fn branch_with_something_in_its_place_does_not_count_as_without_commits() {
        let repo = TempRepo::new();
        fs::create_dir(repo.path().join(".git/refs/heads/main")).unwrap();
        assert!(head_exists(repo.path()).is_err());

        let repo = TempRepo::new();
        fs::write(repo.path().join(".git/refs/heads/foo"), "garbage\n").unwrap();
        repo.git(&["symbolic-ref", "HEAD", "refs/heads/foo/bar"]);
        assert!(head_exists(repo.path()).is_err());
    }

    // With the branch packed `rev-parse` resolves it, so the check alone is tested.
    #[test]
    fn packed_branch_or_detached_head_is_not_a_branch_without_commits() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        repo.git(&["pack-refs", "--all"]);
        assert!(!repo.path().join(".git/refs/heads/main").exists());
        assert_eq!(is_unborn(repo.path()), Ok(false));

        repo.git(&["checkout", "-q", "--detach"]);
        assert_eq!(is_unborn(repo.path()), Ok(false));
    }

    #[test]
    fn in_a_worktree_the_branch_is_looked_up_in_the_common_folder() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        let holder = TempDir::new().unwrap();
        let linked = holder.path().join("wt");
        repo.git(&[
            "worktree",
            "add",
            "-q",
            "-b",
            "wt",
            linked.to_str().unwrap(),
        ]);
        let in_linked = |args: &[&str]| {
            let output = crate::test_support::git_command()
                .current_dir(&linked)
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "{args:?}");
        };
        in_linked(&["checkout", "-q", "--orphan", "new"]);
        assert_eq!(head_exists(&linked), Ok(false));

        for contents in DAMAGED_REFS {
            fs::write(repo.path().join(".git/refs/heads/new"), contents).unwrap();
            assert!(head_exists(&linked).is_err(), "{contents:?}");
        }
    }

    #[test]
    fn head_without_trailing_newline_is_still_a_branch_without_commits() {
        let repo = TempRepo::new();
        fs::write(repo.path().join(".git/HEAD"), "ref: refs/heads/main").unwrap();

        assert_eq!(head_exists(repo.path()), Ok(false));
    }

    #[test]
    fn head_that_is_not_exactly_a_branch_does_not_count_as_without_commits() {
        for contents in [
            "ref: refs/hea",
            "ref: refs/tags/v1\n",
            "ref: refs/heads/\n",
            "ref: refs/heads/ma..in\n",
            "ref: refs/../../x\n",
            "ref: refs/heads/main \n",
            "ref: refs/heads/main\n\n",
            "ref: refs/heads/main\r\n",
            "ref:  refs/heads/main\n",
            "ref:refs/heads/main\n",
        ] {
            let repo = TempRepo::new();
            fs::write(repo.path().join(".git/HEAD"), contents).unwrap();

            let result = head_exists(repo.path());

            assert!(result.is_err(), "{contents:?}: {result:?}");
        }
    }
}
