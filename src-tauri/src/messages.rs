use std::fs;
use std::io;
use std::path::Path;

use crate::storage;

pub const MAX_MESSAGE_HISTORY: usize = 20;
const FILE_NAME: &str = "commit-messages.json";

// Newest to oldest. A history that cannot be read does not prevent working: it is an aid,
// and the next commit writes it again.
pub fn list(dir: &Path) -> Vec<String> {
    fs::read(dir.join(FILE_NAME))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

// A repeated message moves to the top instead of appearing twice.
pub fn push(dir: &Path, message: &str) -> io::Result<()> {
    if message.trim().is_empty() {
        return Ok(());
    }
    let mut messages = list(dir);
    messages.retain(|known| known != message);
    messages.insert(0, message.to_string());
    messages.truncate(MAX_MESSAGE_HISTORY);
    fs::create_dir_all(dir)?;
    let json = serde_json::to_vec_pretty(&messages).map_err(io::Error::other)?;
    storage::write_atomic(&dir.join(FILE_NAME), &json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn empty_history_without_file() {
        let dir = TempDir::new().unwrap();

        assert!(list(dir.path()).is_empty());
    }

    #[test]
    fn history_newest_to_oldest() {
        let dir = TempDir::new().unwrap();

        push(dir.path(), "one").unwrap();
        push(dir.path(), "two").unwrap();

        assert_eq!(list(dir.path()), ["two", "one"]);
    }

    #[test]
    fn history_without_duplicates() {
        let dir = TempDir::new().unwrap();

        push(dir.path(), "one").unwrap();
        push(dir.path(), "two").unwrap();
        push(dir.path(), "one").unwrap();

        assert_eq!(list(dir.path()), ["one", "two"]);
    }

    #[test]
    fn limited_history() {
        let dir = TempDir::new().unwrap();

        for number in 0..21 {
            push(dir.path(), &format!("message {number}")).unwrap();
        }

        let messages = list(dir.path());
        assert_eq!(messages.len(), MAX_MESSAGE_HISTORY);
        assert_eq!(messages.len(), 20);
        assert_eq!(messages[0], "message 20");
        assert_eq!(messages[19], "message 1");
    }

    #[test]
    fn a_blank_message_is_not_saved() {
        let dir = TempDir::new().unwrap();

        push(dir.path(), "  \n").unwrap();

        assert!(list(dir.path()).is_empty());
    }

    #[test]
    fn a_damaged_file_gives_an_empty_history_and_work_can_continue() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(FILE_NAME), "not json").unwrap();

        assert!(list(dir.path()).is_empty());
        push(dir.path(), "one").unwrap();
        assert_eq!(list(dir.path()), ["one"]);
    }
}
