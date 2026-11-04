use std::fs;
use std::io;
use std::path::Path;

use serde_json::{json, Value};

use crate::storage;

const FILE_NAME: &str = "log-options.json";

// They are a convenience: if they cannot be read, it starts with the default values. The frontend
// knows their shape.
pub fn read(dir: &Path) -> Value {
    fs::read(dir.join(FILE_NAME))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(|| json!({}))
}

pub fn write(dir: &Path, options: &Value) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let json = serde_json::to_vec_pretty(options).map_err(io::Error::other)?;
    storage::write_atomic(&dir.join(FILE_NAME), &json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn without_file_there_are_no_options() {
        let dir = TempDir::new().unwrap();

        assert_eq!(read(dir.path()), json!({}));
    }

    #[test]
    fn options_come_back_as_they_were_saved() {
        let dir = TempDir::new().unwrap();
        let options = json!({ "0": { "sort": "linear" } });

        write(dir.path(), &options).unwrap();

        assert_eq!(read(dir.path()), options);
    }

    #[test]
    fn damaged_file_does_not_prevent_working() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(FILE_NAME), "not json").unwrap();

        assert_eq!(read(dir.path()), json!({}));
    }
}
