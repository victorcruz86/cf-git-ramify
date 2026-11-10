// The app was called rama until 2026-11-10. Its data folder, its webview storage and its Keychain
// items carry that name; Ramify copies them the first time it needs them and never moves or
// deletes the originals, so an old build still finds its data.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const OLD_DATA_FOLDER: &str = "dev.rama.app";
pub const NEW_DATA_FOLDER: &str = "dev.ramify.app";
const COPYING_SUFFIX: &str = ".migrating";
const FAILED_SUFFIX: &str = ".migration-failed";
const SET_ASIDE_SUFFIX: &str = ".failed-";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Migration {
    Copied,
    NotNeeded,
}

// `support` is `~/Library/Application Support`: the app's data folder.
pub fn migrate_data_folder(support: &Path) -> io::Result<Migration> {
    migrate_folder(
        &support.join(OLD_DATA_FOLDER),
        &support.join(NEW_DATA_FOLDER),
    )
}

// `library` is `~/Library`. WebKit keeps the webview's storage (localStorage among it) per bundle
// identifier; the folder is copied whole, salts included, without reading its format.
pub fn migrate_webview_storage(library: &Path) -> io::Result<Migration> {
    let webkit = library.join("WebKit");
    migrate_folder(&webkit.join(OLD_DATA_FOLDER), &webkit.join(NEW_DATA_FOLDER))
}

// The copy goes to a folder of its own and is renamed at the end, so a copy that fails or is cut
// short never leaves a partial `new`. A failure leaves a mark beside it: the session that then
// starts with empty data may create `new`, and on the next start that folder is set aside (never
// deleted) and the copy is tried again.
fn migrate_folder(old: &Path, new: &Path) -> io::Result<Migration> {
    if !old.is_dir() {
        return Ok(Migration::NotNeeded);
    }
    let failed = beside(new, FAILED_SUFFIX);
    if new.symlink_metadata().is_ok() {
        if failed.symlink_metadata().is_err() {
            return Ok(Migration::NotNeeded);
        }
        fs::rename(new, free_set_aside_name(new))?;
    }
    let copying = beside(new, COPYING_SUFFIX);
    if copying.symlink_metadata().is_ok() {
        fs::remove_dir_all(&copying)?;
    }
    if let Err(error) = copy_tree(old, &copying).and_then(|()| fs::rename(&copying, new)) {
        let _ = fs::remove_dir_all(&copying);
        let _ = fs::write(&failed, b"");
        return Err(error);
    }
    if failed.symlink_metadata().is_ok() {
        fs::remove_file(&failed)?;
    }
    Ok(Migration::Copied)
}

fn beside(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

fn free_set_aside_name(new: &Path) -> PathBuf {
    (1..)
        .map(|n| beside(new, &format!("{SET_ASIDE_SUFFIX}{n}")))
        .find(|candidate| candidate.symlink_metadata().is_err())
        .expect("a free name")
}

fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let source = entry.path();
        let target = to.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_tree(&source, &target)?;
        } else if kind.is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(&source)?, &target)?;
        } else {
            fs::copy(&source, &target)?;
        }
    }
    Ok(())
}

// The Keychain service rama used for the token of a GitHub server.
pub fn old_token_service(address: &str) -> String {
    format!("rama GitHub {address}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    #[test]
    fn only_the_old_folder_is_copied_whole_and_kept() {
        let support = TempDir::new().unwrap();
        let old = support.path().join(OLD_DATA_FOLDER);
        write(&old.join("settings.json"), "{\"a\":1}");
        write(&old.join("repos/abc/changelists.json"), "[]");
        std::os::unix::fs::symlink("changelists.json", old.join("repos/abc/link")).unwrap();

        assert_eq!(
            migrate_data_folder(support.path()).unwrap(),
            Migration::Copied
        );

        let new = support.path().join(NEW_DATA_FOLDER);
        assert_eq!(read(&new.join("settings.json")), "{\"a\":1}");
        assert_eq!(read(&new.join("repos/abc/changelists.json")), "[]");
        assert_eq!(
            fs::read_link(new.join("repos/abc/link")).unwrap(),
            Path::new("changelists.json")
        );
        assert_eq!(read(&old.join("settings.json")), "{\"a\":1}");
        assert_eq!(read(&old.join("repos/abc/changelists.json")), "[]");
        assert!(!support.path().join("dev.ramify.app.migrating").exists());
    }

    #[test]
    fn with_both_folders_nothing_is_copied() {
        let support = TempDir::new().unwrap();
        write(
            &support.path().join(OLD_DATA_FOLDER).join("settings.json"),
            "old",
        );
        write(
            &support.path().join(NEW_DATA_FOLDER).join("settings.json"),
            "new",
        );

        assert_eq!(
            migrate_data_folder(support.path()).unwrap(),
            Migration::NotNeeded
        );

        assert_eq!(
            read(&support.path().join(NEW_DATA_FOLDER).join("settings.json")),
            "new"
        );
    }

    #[test]
    fn without_the_old_folder_nothing_happens() {
        let support = TempDir::new().unwrap();

        assert_eq!(
            migrate_data_folder(support.path()).unwrap(),
            Migration::NotNeeded
        );

        assert!(!support.path().join(NEW_DATA_FOLDER).exists());
    }

    #[test]
    fn a_failed_copy_leaves_no_new_folder_and_retries_next_time() {
        let support = TempDir::new().unwrap();
        let old = support.path().join(OLD_DATA_FOLDER);
        write(&old.join("a.json"), "a");
        write(&old.join("locked/b.json"), "b");
        let locked = old.join("locked");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        let failed = migrate_data_folder(support.path());
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

        assert!(failed.is_err());
        assert!(!support.path().join(NEW_DATA_FOLDER).exists());
        assert!(!support.path().join("dev.ramify.app.migrating").exists());
        assert_eq!(
            migrate_data_folder(support.path()).unwrap(),
            Migration::Copied
        );
        assert_eq!(
            read(&support.path().join(NEW_DATA_FOLDER).join("locked/b.json")),
            "b"
        );
    }

    #[test]
    fn a_session_after_a_failed_copy_is_set_aside_and_the_copy_retried() {
        let support = TempDir::new().unwrap();
        let old = support.path().join(OLD_DATA_FOLDER);
        write(&old.join("settings.json"), "old");
        write(&old.join("locked/b.json"), "b");
        let locked = old.join("locked");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let failed = migrate_data_folder(support.path());
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(failed.is_err());
        // The session that started with empty data saved something.
        let new = support.path().join(NEW_DATA_FOLDER);
        write(&new.join("settings.json"), "session");

        assert_eq!(
            migrate_data_folder(support.path()).unwrap(),
            Migration::Copied
        );

        assert_eq!(read(&new.join("settings.json")), "old");
        assert_eq!(
            read(
                &support
                    .path()
                    .join("dev.ramify.app.failed-1")
                    .join("settings.json")
            ),
            "session"
        );
        assert!(!support
            .path()
            .join("dev.ramify.app.migration-failed")
            .exists());
        assert_eq!(
            migrate_data_folder(support.path()).unwrap(),
            Migration::NotNeeded
        );
    }

    #[test]
    fn the_webview_storage_is_copied_once_and_kept() {
        let library = TempDir::new().unwrap();
        let old = library.path().join("WebKit").join(OLD_DATA_FOLDER);
        write(&old.join("WebsiteData/Default/salt"), "salt");
        write(
            &old.join("WebsiteData/Default/origin/LocalStorage/localstorage.sqlite3"),
            "keys",
        );

        assert_eq!(
            migrate_webview_storage(library.path()).unwrap(),
            Migration::Copied
        );

        let new = library.path().join("WebKit").join(NEW_DATA_FOLDER);
        assert_eq!(read(&new.join("WebsiteData/Default/salt")), "salt");
        assert_eq!(
            read(&new.join("WebsiteData/Default/origin/LocalStorage/localstorage.sqlite3")),
            "keys"
        );
        assert_eq!(read(&old.join("WebsiteData/Default/salt")), "salt");
        assert_eq!(
            migrate_webview_storage(library.path()).unwrap(),
            Migration::NotNeeded
        );
    }

    #[test]
    fn a_half_copy_left_by_a_crash_is_replaced() {
        let support = TempDir::new().unwrap();
        write(&support.path().join(OLD_DATA_FOLDER).join("a.json"), "a");
        write(
            &support
                .path()
                .join("dev.ramify.app.migrating")
                .join("stale.json"),
            "stale",
        );

        assert_eq!(
            migrate_data_folder(support.path()).unwrap(),
            Migration::Copied
        );

        let new = support.path().join(NEW_DATA_FOLDER);
        assert_eq!(read(&new.join("a.json")), "a");
        assert!(!new.join("stale.json").exists());
    }
}
