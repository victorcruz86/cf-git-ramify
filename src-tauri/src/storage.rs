use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};

// Folder for a repository's own data, stable for the same canonical path.
pub fn repo_dir(base: &Path, repo: &Path) -> PathBuf {
    let canonical = fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
    let digest = Sha256::digest(canonical.as_os_str().as_encoded_bytes());
    let name: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    base.join("repos").join(name)
}

// Sets a damaged file aside without overwriting earlier copies.
pub fn keep_corrupt(dir: &Path, file: &Path, name: &str) -> io::Result<()> {
    let mut target = dir.join(name);
    let mut number = 0;
    while target.symlink_metadata().is_ok() {
        number += 1;
        target = dir.join(format!("{name}.{number}"));
    }
    fs::rename(file, target)
}

// A damaged file is set aside and we carry on with the default values. Any other failure is an
// error: whoever is about to save must not overwrite what it could not read.
pub fn load_json<T: DeserializeOwned + Default>(
    dir: &Path,
    name: &str,
    corrupt_name: &str,
) -> io::Result<T> {
    let file = dir.join(name);
    let bytes = match fs::read(&file) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(T::default()),
        Err(error) => return Err(error),
    };
    match serde_json::from_slice(&bytes) {
        Ok(value) => Ok(value),
        Err(_) => {
            keep_corrupt(dir, &file, corrupt_name)?;
            Ok(T::default())
        }
    }
}

// Writes to a temporary file in the same folder and renames, so we never leave half a file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic_with(path, bytes, None)
}

const TEMP_ATTEMPTS: u32 = 100;

// A name nobody else uses: it belongs to this process, this instant and this call.
fn temp_path(path: &Path) -> io::Result<PathBuf> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let mut name = OsString::from(".");
    name.push(path.file_name().ok_or(io::ErrorKind::InvalidInput)?);
    name.push(format!(
        ".{}-{nanos:x}-{:x}.tmp",
        process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    Ok(path.with_file_name(name))
}

// Same, and with `permissions` the temporary file carries them before replacing the file: the new
// file never exists with another mode. The temporary file is created with `create_new`: it does
// not overwrite a file that already has that name nor write through a link.
pub fn write_atomic_with(
    path: &Path,
    bytes: &[u8],
    permissions: Option<fs::Permissions>,
) -> io::Result<()> {
    let mut attempts = 0;
    let (temp, mut file) = loop {
        let temp = temp_path(path)?;
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
        {
            Ok(file) => break (temp, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                attempts += 1;
                if attempts >= TEMP_ATTEMPTS {
                    return Err(error);
                }
            }
            Err(error) => return Err(error),
        }
    };

    let written = (|| {
        if let Some(permissions) = permissions {
            file.set_permissions(permissions)?;
        }
        file.write_all(bytes)?;
        file.sync_all()
    })();
    drop(file);
    let result = written.and_then(|()| fs::rename(&temp, path));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn repo_dir_is_stable() {
        let base = TempDir::new().unwrap();
        let repo = TempDir::new().unwrap();

        let first = repo_dir(base.path(), repo.path());
        let second = repo_dir(base.path(), repo.path());

        assert_eq!(first, second);
        assert!(first.starts_with(base.path()));
    }

    #[test]
    fn repo_dir_tells_repositories_apart() {
        let base = TempDir::new().unwrap();
        let a = TempDir::new().unwrap();
        let b = TempDir::new().unwrap();

        assert_ne!(
            repo_dir(base.path(), a.path()),
            repo_dir(base.path(), b.path())
        );
    }

    #[test]
    fn repo_dir_uses_the_canonical_path() {
        let base = TempDir::new().unwrap();
        let repo = TempDir::new().unwrap();
        let roundabout = repo
            .path()
            .join(".")
            .join("..")
            .join(repo.path().file_name().unwrap());

        assert_eq!(
            repo_dir(base.path(), repo.path()),
            repo_dir(base.path(), &roundabout)
        );
    }

    #[test]
    fn repo_dir_of_nonexistent_path_does_not_fail() {
        let base = TempDir::new().unwrap();
        let missing = base.path().join("does-not-exist");
        assert_eq!(
            repo_dir(base.path(), &missing),
            repo_dir(base.path(), &missing)
        );
    }

    #[test]
    fn atomic_write_creates_and_replaces() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("data.json");

        write_atomic(&file, b"one").unwrap();
        write_atomic(&file, b"two").unwrap();

        assert_eq!(fs::read(&file).unwrap(), b"two");
    }

    #[test]
    fn atomic_write_leaves_no_temporaries() {
        let dir = TempDir::new().unwrap();
        write_atomic(&dir.path().join("data.json"), b"one").unwrap();

        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();

        assert_eq!(names, ["data.json"]);
    }

    #[test]
    fn atomic_write_with_permissions_keeps_them() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("script.sh");
        fs::write(&file, b"one").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
        let permissions = fs::metadata(&file).unwrap().permissions();

        write_atomic_with(&file, b"two", Some(permissions)).unwrap();

        assert_eq!(fs::read(&file).unwrap(), b"two");
        let mode = fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }

    #[test]
    fn atomic_write_does_not_overwrite_a_file_with_the_temporary_name() {
        let dir = TempDir::new().unwrap();
        let tracked = dir.path().join(".a.txt.tmp");
        fs::write(&tracked, b"mine").unwrap();

        write_atomic(&dir.path().join("a.txt"), b"data").unwrap();

        assert_eq!(fs::read(&tracked).unwrap(), b"mine");
        assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"data");
    }

    #[test]
    fn atomic_write_does_not_write_through_a_link() {
        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let target = outside.path().join("foreign");
        fs::write(&target, b"foreign").unwrap();
        let link = dir.path().join(".a.txt.tmp");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        write_atomic(&dir.path().join("a.txt"), b"data").unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"foreign");
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(fs::read(dir.path().join("a.txt")).unwrap(), b"data");
    }

    #[test]
    fn atomic_write_with_permissions_does_not_overwrite_the_temporary_name() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(".a.txt.tmp"), b"mine").unwrap();
        let permissions = fs::metadata(dir.path().join(".a.txt.tmp"))
            .unwrap()
            .permissions();

        write_atomic_with(&dir.path().join("a.txt"), b"data", Some(permissions)).unwrap();

        assert_eq!(fs::read(dir.path().join(".a.txt.tmp")).unwrap(), b"mine");
    }

    #[test]
    fn atomic_write_in_nonexistent_folder_fails() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("missing").join("data.json");
        assert!(write_atomic(&file, b"one").is_err());
    }
}
