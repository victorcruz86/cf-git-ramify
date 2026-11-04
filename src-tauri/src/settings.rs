use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::merge::MergeOption;
use crate::merge3::Whitespace;
use crate::storage;

const FILE_NAME: &str = "settings.json";
const CORRUPT_FILE_NAME: &str = "settings.json.corrupt";
const REPO_FILE_NAME: &str = "repo-settings.json";
const REPO_CORRUPT_FILE_NAME: &str = "repo-settings.json.corrupt";

// `GitIncomingCheckStrategy`: whether the remotes are asked for new commits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IncomingCheck {
    #[default]
    Auto,
    Always,
    Never,
}

// `UpdateMethod`, without `BRANCH_DEFAULT`: IntelliJ's interface does not offer it either.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateMethod {
    #[default]
    Merge,
    Rebase,
}

// The settings of `Settings > Version Control > Git`. They belong to the app, not to each
// repository. Whatever is missing from the file takes its default value, and unknown things are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub git_path: Option<String>,
    pub staging_area: bool,
    pub warn_crlf: bool,
    pub warn_detached: bool,
    // `WARN_ABOUT_LARGE_FILES` and `WARN_ABOUT_LARGE_FILES_LIMIT_MB`.
    pub warn_large_files: bool,
    pub large_files_limit_mb: u32,
    // `WARN_ABOUT_BAD_FILE_NAMES`.
    pub warn_bad_file_names: bool,
    pub sign_commits: bool,
    pub signing_key: Option<String>,
    // IntelliJ turns it off by default because it stores in Password Safe. Here the credentials
    // stay in the system: without the helper, git would ask for them on every command.
    pub use_credential_helper: bool,
    pub incoming_check: IncomingCheck,
    pub update_method: UpdateMethod,
    // `Don't show again` of the Update Project dialog.
    pub show_update_dialog: bool,
    // `Filter "Update Project" information by paths:`, relative to the root. Empty: all.
    pub update_paths: Vec<String>,
    // `Auto-update if push of the current branch was rejected`.
    pub push_auto_update: bool,
    // `Show Push dialog for Commit and Push`.
    pub preview_push_on_commit: bool,
    // `Show Push dialog only when committing to protected branches`.
    pub preview_push_protected_only: bool,
    // `Automatically apply non-conflicting changes` of the merge window.
    pub merge_auto_apply: bool,
    // The whitespace policy of the merge window, which is remembered as in IntelliJ.
    pub merge_whitespace: Whitespace,
    // `Add the 'cherry-picked from <hash>' suffix when picking commits pushed to protected branches`.
    pub cherry_pick_suffix: bool,
    // `Use non-modal commit interface` (`COMMIT_FROM_LOCAL_CHANGES`). Off: the commit is a dialog.
    pub non_modal_commit: bool,
    // `Clear initial commit message` (`CLEAR_INITIAL_COMMIT_MESSAGE`).
    pub clear_initial_commit_message: bool,
}

impl Settings {
    // `Enable staging area` only counts in the non-modal interface (`canEnableStagingArea`): while modal it
    // stays saved but is not in effect. The TS side has the same rule in `effectiveStaging`.
    pub fn effective_staging(&self) -> bool {
        self.non_modal_commit && self.staging_area
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            git_path: None,
            staging_area: false,
            warn_crlf: true,
            warn_detached: true,
            warn_large_files: true,
            large_files_limit_mb: 50,
            warn_bad_file_names: true,
            sign_commits: false,
            signing_key: None,
            use_credential_helper: true,
            incoming_check: IncomingCheck::Auto,
            update_method: UpdateMethod::Merge,
            show_update_dialog: true,
            update_paths: Vec::new(),
            push_auto_update: false,
            preview_push_on_commit: true,
            preview_push_protected_only: false,
            merge_auto_apply: false,
            merge_whitespace: Whitespace::Default,
            cherry_pick_suffix: true,
            non_modal_commit: false,
            clear_initial_commit_message: false,
        }
    }
}

// What IntelliJ stores in `.idea/vcs.xml` and `PushSettings`: here each repository has its own folder.
// `force_push_targets` are the `Don't warn about this target` ones, as `remote:branch`: the colon
// fits in neither of the two names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RepoSettings {
    pub protected_branches: Vec<String>,
    pub force_push_targets: Vec<String>,
    pub merge: MergeSettings,
    pub rebase: RebaseSettings,
}

impl Default for RepoSettings {
    fn default() -> Self {
        Self {
            protected_branches: ["master", "main"].map(String::from).to_vec(),
            force_push_targets: Vec::new(),
            merge: MergeSettings::default(),
            rebase: RebaseSettings::default(),
        }
    }
}

// `GitMergeSettings`: the branch and the options of the last Merge…, without the message.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MergeSettings {
    pub branch: Option<String>,
    pub options: Vec<MergeOption>,
}

// The flags of `GitRebaseSettings`, with the name the interface gives them. `--onto` and the branch to rebase
// are not remembered, as in `REBASE_FLAGS`.
pub const REBASE_FLAGS: [&str; 4] = ["rebaseMerges", "keepEmpty", "root", "interactive"];

// `GitRebaseSettings`: the flags and the base (the new one if there was `--onto`, otherwise the upstream).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RebaseSettings {
    pub options: Vec<String>,
    pub new_base: Option<String>,
}

fn filled(text: Option<String>) -> Option<String> {
    text.map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn clean_paths(paths: Vec<String>) -> Vec<String> {
    let mut clean: Vec<String> = Vec::new();
    for path in paths.iter().map(|path| path.trim()) {
        if !path.is_empty() && !clean.iter().any(|seen| seen == path) {
            clean.push(path.to_string());
        }
    }
    clean
}

impl Settings {
    fn normalized(self) -> Self {
        Self {
            git_path: filled(self.git_path),
            signing_key: filled(self.signing_key),
            update_paths: clean_paths(self.update_paths),
            ..self
        }
    }
}

fn try_load(dir: &Path) -> io::Result<Settings> {
    storage::load_json::<Settings>(dir, FILE_NAME, CORRUPT_FILE_NAME).map(Settings::normalized)
}

// It never prevents starting: whatever cannot be read leaves the default values.
pub fn load(dir: &Path) -> Settings {
    try_load(dir).unwrap_or_default()
}

pub fn save(dir: &Path, settings: &Settings) -> io::Result<()> {
    try_load(dir)?;
    fs::create_dir_all(dir)?;
    let json =
        serde_json::to_vec_pretty(&settings.clone().normalized()).map_err(io::Error::other)?;
    storage::write_atomic(&dir.join(FILE_NAME), &json)
}

// Whoever protects something uses this one: what cannot be read cannot be treated as if nothing were
// protected.
pub fn try_load_repo(dir: &Path) -> io::Result<RepoSettings> {
    storage::load_json(dir, REPO_FILE_NAME, REPO_CORRUPT_FILE_NAME)
}

pub fn load_repo(dir: &Path) -> RepoSettings {
    try_load_repo(dir).unwrap_or_default()
}

fn save_repo(dir: &Path, settings: &RepoSettings) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let json = serde_json::to_vec_pretty(settings).map_err(io::Error::other)?;
    storage::write_atomic(&dir.join(REPO_FILE_NAME), &json)
}

pub fn save_protected_branches(dir: &Path, patterns: Vec<String>) -> io::Result<()> {
    let protected_branches = clean_paths(patterns);
    let settings = RepoSettings {
        protected_branches,
        ..try_load_repo(dir)?
    };
    save_repo(dir, &settings)
}

// The options without repeats and in the order they arrived.
fn unique<T: PartialEq>(items: Vec<T>) -> Vec<T> {
    let mut clean: Vec<T> = Vec::new();
    for item in items {
        if !clean.contains(&item) {
            clean.push(item);
        }
    }
    clean
}

pub fn save_merge(dir: &Path, branch: Option<String>, options: Vec<MergeOption>) -> io::Result<()> {
    let merge = MergeSettings {
        branch: filled(branch),
        options: unique(options),
    };
    save_repo(
        dir,
        &RepoSettings {
            merge,
            ..try_load_repo(dir)?
        },
    )
}

pub fn save_rebase(dir: &Path, options: Vec<String>, new_base: Option<String>) -> io::Result<()> {
    let rebase = RebaseSettings {
        options: REBASE_FLAGS
            .iter()
            .filter(|flag| options.iter().any(|option| option == *flag))
            .map(|flag| flag.to_string())
            .collect(),
        new_base: filled(new_base),
    };
    save_repo(
        dir,
        &RepoSettings {
            rebase,
            ..try_load_repo(dir)?
        },
    )
}

pub fn remember_force_target(dir: &Path, remote: &str, branch: &str) -> io::Result<()> {
    let mut settings = try_load_repo(dir)?;
    let target = format!("{remote}:{branch}");
    if !settings.force_push_targets.contains(&target) {
        settings.force_push_targets.push(target);
    }
    save_repo(dir, &settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    #[test]
    fn default_values() {
        let settings = Settings::default();

        assert!(!settings.staging_area);
        assert!(settings.warn_crlf);
        assert!(settings.warn_detached);
        // `GitVcsOptions`: `WARN_ABOUT_LARGE_FILES = true`, the limit at 50 MB and `WARN_ABOUT_BAD_FILE_NAMES = true`.
        assert!(settings.warn_large_files);
        assert_eq!(settings.large_files_limit_mb, 50);
        assert!(settings.warn_bad_file_names);
        assert!(!settings.sign_commits);
        assert!(settings.use_credential_helper);
        assert_eq!(settings.incoming_check, IncomingCheck::Auto);
        assert_eq!(settings.update_method, UpdateMethod::Merge);
        assert!(settings.show_update_dialog);
        assert!(settings.update_paths.is_empty());
        assert!(!settings.push_auto_update);
        assert!(settings.preview_push_on_commit);
        assert!(!settings.preview_push_protected_only);
        assert_eq!(settings.git_path, None);
        assert_eq!(settings.signing_key, None);
        // `TextDiffSettingsHolder`: `MERGE_AUTO_APPLY_NON_CONFLICTED_CHANGES = false` e
        // `IGNORE_POLICY = DEFAULT`.
        assert!(!settings.merge_auto_apply);
        assert_eq!(settings.merge_whitespace, Whitespace::Default);
        // `ADD_SUFFIX_TO_CHERRY_PICKS_OF_PUBLISHED_COMMITS = true`.
        assert!(settings.cherry_pick_suffix);
    }

    #[test]
    fn non_modal_commit_and_clear_message_default_to_false() {
        let settings = Settings::default();

        // `COMMIT_FROM_LOCAL_CHANGES` and `CLEAR_INITIAL_COMMIT_MESSAGE`: the commit is modal by default.
        assert!(!settings.non_modal_commit);
        assert!(!settings.clear_initial_commit_message);
    }

    #[test]
    fn a_settings_file_without_the_new_fields_reads_modal() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join(FILE_NAME),
            r#"{"stagingArea": true, "warnCrlf": false}"#,
        )
        .unwrap();

        let settings = load(dir.path());

        assert!(!settings.non_modal_commit);
        assert!(!settings.clear_initial_commit_message);
        // What the file did say is kept, including the staging area: it stays saved while modal.
        assert!(settings.staging_area);
        assert!(!settings.warn_crlf);
    }

    #[test]
    fn staging_is_effective_only_in_non_modal_mode() {
        let with = |non_modal_commit, staging_area| Settings {
            non_modal_commit,
            staging_area,
            ..Default::default()
        };

        assert!(!with(false, true).effective_staging());
        assert!(!with(false, false).effective_staging());
        assert!(!with(true, false).effective_staging());
        assert!(with(true, true).effective_staging());
    }

    #[test]
    fn without_a_file_gives_the_default_values_and_creates_nothing() {
        let dir = TempDir::new().unwrap();

        assert_eq!(load(dir.path()), Settings::default());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn save_and_read() {
        let dir = TempDir::new().unwrap();
        let settings = Settings {
            git_path: Some("/opt/git".to_string()),
            staging_area: true,
            warn_crlf: false,
            warn_detached: false,
            warn_large_files: false,
            large_files_limit_mb: 7,
            warn_bad_file_names: false,
            sign_commits: true,
            signing_key: Some("ABC123".to_string()),
            use_credential_helper: false,
            incoming_check: IncomingCheck::Never,
            update_method: UpdateMethod::Rebase,
            show_update_dialog: false,
            update_paths: vec!["src".to_string(), "docs/a.md".to_string()],
            push_auto_update: true,
            preview_push_on_commit: false,
            preview_push_protected_only: true,
            merge_auto_apply: true,
            merge_whitespace: Whitespace::Ignore,
            cherry_pick_suffix: false,
            non_modal_commit: true,
            clear_initial_commit_message: true,
        };

        save(dir.path(), &settings).unwrap();

        assert_eq!(load(dir.path()), settings);
    }

    #[test]
    fn save_creates_the_folder() {
        let dir = TempDir::new().unwrap();
        let nested = dir.path().join("a/b");

        save(&nested, &Settings::default()).unwrap();

        assert!(nested.join(FILE_NAME).is_file());
    }

    #[test]
    fn missing_or_unknown_fields_use_the_default_values() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join(FILE_NAME),
            r#"{"stagingArea": true, "fromFuture": 3}"#,
        )
        .unwrap();

        let settings = load(dir.path());

        assert!(settings.staging_area);
        assert!(settings.warn_crlf);
        assert!(settings.warn_detached);
        assert!(settings.warn_large_files);
        assert_eq!(settings.large_files_limit_mb, 50);
        assert!(settings.warn_bad_file_names);
        assert!(settings.use_credential_helper);
    }

    #[test]
    fn blank_texts_count_as_no_value() {
        let dir = TempDir::new().unwrap();
        let settings = Settings {
            git_path: Some("  ".to_string()),
            signing_key: Some(String::new()),
            ..Default::default()
        };

        save(dir.path(), &settings).unwrap();

        let read = load(dir.path());
        assert_eq!(read.git_path, None);
        assert_eq!(read.signing_key, None);
    }

    #[test]
    fn the_update_filter_paths_are_cleaned_when_saving() {
        let dir = TempDir::new().unwrap();
        let settings = Settings {
            update_paths: ["  src ", "", "src", "   ", "-odd", "docs"]
                .map(String::from)
                .to_vec(),
            ..Default::default()
        };

        save(dir.path(), &settings).unwrap();

        assert_eq!(load(dir.path()).update_paths, ["src", "-odd", "docs"]);
    }

    #[test]
    fn a_damaged_file_is_kept_and_does_not_prevent_starting() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(FILE_NAME), b"{ this is not json").unwrap();

        assert_eq!(load(dir.path()), Settings::default());

        assert!(!dir.path().join(FILE_NAME).exists());
        assert_eq!(
            fs::read(dir.path().join(CORRUPT_FILE_NAME)).unwrap(),
            b"{ this is not json"
        );
    }

    #[test]
    fn a_damaged_file_does_not_overwrite_another_ones_copy() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(CORRUPT_FILE_NAME), b"first").unwrap();
        fs::write(dir.path().join(FILE_NAME), b"second").unwrap();

        load(dir.path());

        let second = format!("{CORRUPT_FILE_NAME}.1");
        assert_eq!(
            fs::read(dir.path().join(CORRUPT_FILE_NAME)).unwrap(),
            b"first"
        );
        assert_eq!(fs::read(dir.path().join(second)).unwrap(), b"second");
    }

    #[test]
    fn an_unreadable_file_is_not_overwritten_when_saving() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join(FILE_NAME);
        fs::write(&file, b"{}").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();

        assert_eq!(load(dir.path()), Settings::default());
        let result = save(
            dir.path(),
            &Settings {
                staging_area: true,
                ..Default::default()
            },
        );

        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(result.is_err());
        assert_eq!(fs::read(&file).unwrap(), b"{}");
    }
    // Per repository.

    #[test]
    fn by_default_protects_master_and_main_and_no_silent_target() {
        let dir = TempDir::new().unwrap();

        let settings = load_repo(dir.path());

        assert_eq!(settings.protected_branches, ["master", "main"]);
        assert!(settings.force_push_targets.is_empty());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn each_change_keeps_the_rest() {
        let dir = TempDir::new().unwrap();

        remember_force_target(dir.path(), "origin", "feature").unwrap();
        remember_force_target(dir.path(), "origin", "feature").unwrap();
        save_protected_branches(
            dir.path(),
            ["  release.* ", "", "release.*"].map(String::from).to_vec(),
        )
        .unwrap();

        let settings = load_repo(dir.path());
        assert_eq!(settings.protected_branches, ["release.*"]);
        assert_eq!(settings.force_push_targets, ["origin:feature"]);
    }

    #[test]
    fn leaving_the_list_empty_does_not_go_back_to_the_default_values() {
        let dir = TempDir::new().unwrap();

        save_protected_branches(dir.path(), Vec::new()).unwrap();

        assert!(load_repo(dir.path()).protected_branches.is_empty());
    }

    #[test]
    fn a_damaged_repository_file_is_kept_and_protects_the_built_in_ones() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(REPO_FILE_NAME), b"{ broken").unwrap();

        assert_eq!(load_repo(dir.path()), RepoSettings::default());
        assert_eq!(
            fs::read(dir.path().join(REPO_CORRUPT_FILE_NAME)).unwrap(),
            b"{ broken"
        );
    }

    #[test]
    fn an_unreadable_repository_file_is_an_error_and_is_not_overwritten() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join(REPO_FILE_NAME);
        fs::write(&file, b"{}").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();

        let read = try_load_repo(dir.path());
        let saved = save_protected_branches(dir.path(), Vec::new());

        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read.is_err());
        assert!(saved.is_err());
        assert_eq!(fs::read(&file).unwrap(), b"{}");
    }

    #[test]
    fn merge_and_rebase_start_empty_and_an_old_file_leaves_them_that_way() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join(REPO_FILE_NAME),
            r#"{"protectedBranches": ["release"], "forcePushTargets": []}"#,
        )
        .unwrap();

        let settings = load_repo(dir.path());

        assert_eq!(settings.protected_branches, ["release"]);
        assert_eq!(settings.merge, MergeSettings::default());
        assert_eq!(settings.merge.branch, None);
        assert!(settings.merge.options.is_empty());
        assert_eq!(settings.rebase, RebaseSettings::default());
        assert!(settings.rebase.options.is_empty());
        assert_eq!(settings.rebase.new_base, None);
    }

    #[test]
    fn merge_and_rebase_are_remembered_per_repository_without_clobbering_the_rest() {
        let dir = TempDir::new().unwrap();
        save_protected_branches(dir.path(), vec!["release".to_string()]).unwrap();
        remember_force_target(dir.path(), "origin", "feature").unwrap();

        save_merge(
            dir.path(),
            Some("  feature  ".to_string()),
            vec![MergeOption::NoFf, MergeOption::NoVerify, MergeOption::NoFf],
        )
        .unwrap();
        save_rebase(
            dir.path(),
            ["interactive", "odd", "rebaseMerges", "interactive", "root"]
                .map(String::from)
                .to_vec(),
            Some(" origin/main ".to_string()),
        )
        .unwrap();

        let settings = load_repo(dir.path());
        assert_eq!(settings.merge.branch.as_deref(), Some("feature"));
        assert_eq!(
            settings.merge.options,
            [MergeOption::NoFf, MergeOption::NoVerify]
        );
        // The flags come back in the order of `GitRebaseOption`, and only the ones that exist.
        assert_eq!(
            settings.rebase.options,
            ["rebaseMerges", "root", "interactive"]
        );
        assert_eq!(settings.rebase.new_base.as_deref(), Some("origin/main"));
        assert_eq!(settings.protected_branches, ["release"]);
        assert_eq!(settings.force_push_targets, ["origin:feature"]);
    }

    #[test]
    fn saving_protected_or_a_target_keeps_merge_and_rebase() {
        let dir = TempDir::new().unwrap();
        save_merge(dir.path(), Some("x".to_string()), vec![MergeOption::Squash]).unwrap();
        save_rebase(dir.path(), vec!["keepEmpty".to_string()], None).unwrap();

        save_protected_branches(dir.path(), Vec::new()).unwrap();
        remember_force_target(dir.path(), "origin", "a").unwrap();

        let settings = load_repo(dir.path());
        assert_eq!(settings.merge.branch.as_deref(), Some("x"));
        assert_eq!(settings.rebase.options, ["keepEmpty"]);
    }

    #[test]
    fn a_blank_text_clears_the_remembered_branch_and_base() {
        let dir = TempDir::new().unwrap();
        save_merge(dir.path(), Some("x".to_string()), Vec::new()).unwrap();
        save_rebase(dir.path(), Vec::new(), Some("main".to_string())).unwrap();

        save_merge(dir.path(), Some("  ".to_string()), Vec::new()).unwrap();
        save_rebase(dir.path(), Vec::new(), Some(String::new())).unwrap();

        let settings = load_repo(dir.path());
        assert_eq!(settings.merge.branch, None);
        assert_eq!(settings.rebase.new_base, None);
    }

    #[test]
    fn saving_merge_or_rebase_does_not_overwrite_an_unreadable_file() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join(REPO_FILE_NAME);
        fs::write(&file, b"{}").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();

        let merge = save_merge(dir.path(), None, Vec::new());
        let rebase = save_rebase(dir.path(), Vec::new(), None);

        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(merge.is_err());
        assert!(rebase.is_err());
        assert_eq!(fs::read(&file).unwrap(), b"{}");
    }
}
