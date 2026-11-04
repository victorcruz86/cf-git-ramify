// It runs in its own process: the configured path is the whole app's and would change the other tests.
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use rama_lib::commands::ApiError;
use rama_lib::git::{self, GitError};
use tempfile::TempDir;

fn wrapper(dir: &Path, log: &Path) -> String {
    let real =
        String::from_utf8(Command::new("which").arg("git").output().unwrap().stdout).unwrap();
    let script = dir.join("git-wrapped");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\necho \"$1\" >> '{}'\nexec '{}' \"$@\"\n",
            log.display(),
            real.trim()
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    script.to_str().unwrap().to_string()
}

#[test]
fn configured_git_path() {
    let tools = TempDir::new().unwrap();
    let repo = TempDir::new().unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo.path())
        .status()
        .unwrap()
        .success());
    let log = tools.path().join("calls");
    let script = wrapper(tools.path(), &log);

    git::set_binary(Some(&script));
    git::run(repo.path(), &["rev-parse", "--git-dir"]).unwrap();
    git::run_with_input(repo.path(), &["hash-object", "--stdin"], b"hello").unwrap();
    git::stream(repo.path(), &["rev-parse", "--git-dir"], b'\n', |_| {}).unwrap();
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        "rev-parse\nhash-object\nrev-parse\n"
    );

    // Without restarting.
    git::set_binary(Some("/does/not/exist/git"));
    let error = git::run(repo.path(), &["status"]).unwrap_err();
    assert_eq!(error, GitError::NotInstalled);
    let api = ApiError::from(error);
    assert_eq!(api.kind, "git_not_found");
    assert!(api.message.contains("/does/not/exist/git"), "{}", api.message);

    // A folder or a file without execute permission exist, but cannot be launched.
    let plain = tools.path().join("no-permission");
    fs::write(&plain, "#!/bin/sh\n").unwrap();
    fs::set_permissions(&plain, fs::Permissions::from_mode(0o644)).unwrap();
    for path in [tools.path(), plain.as_path()] {
        git::set_binary(path.to_str());
        let message = ApiError::from(git::run(repo.path(), &["status"]).unwrap_err()).message;
        assert!(message.contains(path.to_str().unwrap()), "{message}");
        let message = ApiError::from(git::version_of(path.to_str()).unwrap_err()).message;
        assert!(message.contains(path.to_str().unwrap()), "{message}");
    }

    git::set_binary(None);
    git::run(repo.path(), &["status"]).unwrap();
    assert_eq!(fs::read_to_string(&log).unwrap().lines().count(), 3);
}

#[test]
fn version_of_a_path_without_configuring_it() {
    let tools = TempDir::new().unwrap();
    let script = wrapper(tools.path(), &tools.path().join("calls"));
    let version = git::version_of(Some(&script)).unwrap();
    assert!(
        version.starts_with(|first: char| first.is_ascii_digit()),
        "{version}"
    );

    assert_eq!(
        git::version_of(Some("/does/not/exist/git")).unwrap_err(),
        GitError::NotInstalled
    );

    let other = tools.path().join("another");
    fs::write(&other, "#!/bin/sh\necho hello\n").unwrap();
    fs::set_permissions(&other, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        git::version_of(other.to_str()),
        Err(GitError::Io(message)) if message.contains("Cannot identify version")
    ));
}
