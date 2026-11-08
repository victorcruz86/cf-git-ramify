// What the tests of the reader, the writer and the builder share.
use std::fs;
use std::path::{Path, PathBuf};

use super::*;

pub(super) fn fixtures(dir: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/patch")
        .join(dir)
}

pub(super) fn cases(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.file_type().unwrap().is_dir())
        .map(|entry| entry.file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

pub(super) fn options(base: &Path) -> WriteOptions {
    WriteOptions {
        include_base: false,
        reverse: false,
        base_path: base.to_path_buf(),
    }
}

pub(super) fn text(file: &FilePatch) -> &TextFilePatch {
    match file {
        FilePatch::Text(text) => text,
        FilePatch::Binary(_) => panic!("a binary file patch"),
    }
}

pub(super) fn modified(path: &str, before: &[u8], after: &[u8]) -> PatchSource {
    PatchSource {
        before: Some((path.to_string(), before.to_vec())),
        after: Some((path.to_string(), after.to_vec())),
        before_rev: Some("1".to_string()),
        binary: false,
    }
}

pub(super) fn text_of(patch: &Patch) -> &TextFilePatch {
    text(&patch.files[0])
}
