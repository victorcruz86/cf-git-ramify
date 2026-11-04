use std::fs;
use std::io;
use std::path::Path;

use crate::storage;

// `VcsConfiguration.MAX_STORED_MESSAGES`.
pub const MAX_MESSAGE_HISTORY: usize = 25;
const FILE_NAME: &str = "commit-messages.json";
const LAST_FILE_NAME: &str = "last-commit-message.json";

// Newest to oldest. A history that cannot be read does not prevent working: it is an aid,
// and the next commit writes it again.
pub fn list(dir: &Path) -> Vec<String> {
    fs::read(dir.join(FILE_NAME))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

// `LAST_COMMIT_MESSAGE`: the message of the last commit made through the app. It has its own
// file, so the history, which is only an aid, can be lost or truncated without losing it.
pub fn last_commit_message(dir: &Path) -> Option<String> {
    fs::read(dir.join(LAST_FILE_NAME))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

// A repeated message moves to the top instead of appearing twice.
pub fn push(dir: &Path, message: &str) -> io::Result<()> {
    if message.trim().is_empty() {
        return Ok(());
    }
    fs::create_dir_all(dir)?;
    let last = serde_json::to_vec(message).map_err(io::Error::other)?;
    storage::write_atomic(&dir.join(LAST_FILE_NAME), &last)?;
    let mut messages = list(dir);
    messages.retain(|known| known != message);
    messages.insert(0, message.to_string());
    messages.truncate(MAX_MESSAGE_HISTORY);
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
    fn history_keeps_twenty_five_messages() {
        let dir = TempDir::new().unwrap();

        for number in 0..26 {
            push(dir.path(), &format!("message {number}")).unwrap();
        }

        let messages = list(dir.path());
        assert_eq!(MAX_MESSAGE_HISTORY, 25);
        assert_eq!(messages.len(), 25);
        assert_eq!(messages[0], "message 25");
        assert_eq!(messages[24], "message 1");
    }

    #[test]
    fn the_last_message_is_kept_apart_from_the_history() {
        let dir = TempDir::new().unwrap();
        assert_eq!(last_commit_message(dir.path()), None);

        push(dir.path(), "one").unwrap();
        push(dir.path(), "two").unwrap();
        push(dir.path(), "one").unwrap();
        push(dir.path(), "  \n").unwrap();

        // A blank message changes neither of them, and a repeated one is the last again.
        assert_eq!(last_commit_message(dir.path()).as_deref(), Some("one"));
        // Losing the history does not lose the last message.
        std::fs::write(dir.path().join(FILE_NAME), "not json").unwrap();
        assert!(list(dir.path()).is_empty());
        assert_eq!(last_commit_message(dir.path()).as_deref(), Some("one"));
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
