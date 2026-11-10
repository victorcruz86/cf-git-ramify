// It runs in its own process: it changes the environment so as not to touch anyone's global configuration.
use std::env;
use std::fs;

use ramify_lib::commit;
use tempfile::TempDir;

#[test]
fn fix_and_commit_changes_the_global_configuration() {
    let home = TempDir::new().unwrap();
    let repo = TempDir::new().unwrap();
    let config = home.path().join("gitconfig");
    env::set_var("GIT_CONFIG_GLOBAL", &config);

    commit::fix_autocrlf(repo.path()).unwrap();

    let written = fs::read_to_string(config).unwrap();
    assert!(written.contains("autocrlf = input"), "{written}");
}
