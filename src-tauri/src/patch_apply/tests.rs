use super::*;
use crate::test_support::RepoHarness;
use std::collections::BTreeMap;
use std::path::PathBuf;

// Finding 3: new files whose names read as pathspec magic or a glob still reach the index,
// and an ignored one stays out.
#[test]
fn new_files_named_like_pathspecs_are_added_and_ignored_ones_are_not() {
    let harness = RepoHarness::new();
    harness.repo.commit(".gitignore", b"*.log\n", "ignore");
    let added: Vec<String> = [":(glob)x", "*.txt", "a.txt", "debug.log"]
        .into_iter()
        .map(str::to_string)
        .collect();
    for path in &added {
        harness.write(path, "new\n");
    }

    assert_eq!(
        ignored(harness.path(), &added).unwrap(),
        BTreeSet::from(["debug.log".to_string()])
    );
    let messages = update_index(
        harness.path(),
        &IndexChanges {
            added,
            removed: Vec::new(),
        },
    );

    assert_eq!(messages, Vec::<String>::new());
    let staged = harness.git(&["diff", "--cached", "--name-only", "-z"]);
    let mut staged: Vec<&str> = staged.split('\0').filter(|path| !path.is_empty()).collect();
    staged.sort();
    assert_eq!(staged, ["*.txt", ":(glob)x", "a.txt"]);
}

fn fixtures(dir: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/patch")
        .join(dir)
}

fn read_patch(text: &str) -> Patch {
    patch::read(text.as_bytes()).unwrap()
}

// What the dialog sends back: every planned file, with the hashes the plan read.
fn planned_files(
    repo: &Path,
    patch: &Patch,
    mapping: &PathMapping,
) -> (Vec<String>, BTreeMap<String, String>) {
    let plan = plan(repo, patch, mapping);
    let selected = plan.files.iter().map(|file| file.path.clone()).collect();
    let expected = plan
        .files
        .into_iter()
        .flat_map(|file| file.hashes)
        .collect();
    (selected, expected)
}

fn none() -> BTreeMap<String, String> {
    BTreeMap::new()
}

fn apply_all(harness: &RepoHarness, text: &str) -> ApplyResult {
    apply_with(harness, text, &PathMapping::default(), &[])
}

fn apply_with(
    harness: &RepoHarness,
    text: &str,
    mapping: &PathMapping,
    overwrite: &[&str],
) -> ApplyResult {
    let patch = read_patch(text);
    let (selected, expected) = planned_files(harness.path(), &patch, mapping);
    let overwrite: Vec<String> = overwrite.iter().map(|path| path.to_string()).collect();
    let mapping = mapping.clone();
    harness
        .run(move |repo| apply(repo, &patch, &mapping, &selected, &overwrite, &expected))
        .unwrap()
}

fn stripping(count: usize, files: usize) -> PathMapping {
    PathMapping {
        files: vec![
            FileMapping {
                strip: count,
                base: String::new(),
            };
            files
        ],
    }
}

fn numbered(lines: std::ops::RangeInclusive<u32>) -> String {
    lines.map(|line| format!("line {line}\n")).collect()
}

// Every file under `dir` but `.git`, with its bytes.
fn tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_name() == ".git" {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let name = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                out.insert(name, fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

fn copy_tree(from: &Path, to: &Path, rename: &dyn Fn(&str) -> String) {
    for (name, bytes) in tree(from) {
        let target = to.join(rename(&name));
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, bytes).unwrap();
    }
}

// `ApplyPatchTest.useCyrillicNames`.
fn cyrillic(name: &str) -> String {
    name.replace("cyr1.txt", "имя файла.txt")
        .replace("cyr2.txt", "кириллица.txt")
}

#[test]
fn status_and_follows_intellij() {
    use ApplyStatus::*;
    // `ApplyPatchStatusTest.checkStatusAnd`.
    let ordered = [Skip, Success, AlreadyApplied, Partial, Failure, Abort];
    for (a, lhs) in ordered.iter().enumerate() {
        for (b, rhs) in ordered.iter().enumerate() {
            let expected = if lhs == rhs {
                *lhs
            } else if [*lhs, *rhs]
                .iter()
                .all(|s| [Success, AlreadyApplied].contains(s))
            {
                Partial
            } else {
                ordered[a.max(b)]
            };
            assert_eq!(ApplyStatus::and(Some(*lhs), Some(*rhs)), Some(expected));
        }
        assert_eq!(ApplyStatus::and(Some(*lhs), None), Some(*lhs));
        assert_eq!(ApplyStatus::and(None, Some(*lhs)), Some(*lhs));
    }
    assert_eq!(ApplyStatus::and(None, None), None);
}

// The cases `ApplyPatchTest` runs that rama does not apply as IntelliJ, with the reason and
// what rama does instead.
enum Skipped {
    // Decision 4: the context-diff format is not read.
    ContextDiff,
    // Decision 4: IntelliJ's `GenericPatchApplier` splits the hunk and finds one block
    // already applied and the other applicable (PARTIAL). rama tries whole hunks only, finds
    // neither, and with no base the file fails and is left as it was.
    GenericFuzz,
}

const SKIPPED: [(&str, Skipped); 7] = [
    ("contextDiff", Skipped::ContextDiff),
    ("contextDiffAddLine", Skipped::ContextDiff),
    ("contextDiffMultiFile", Skipped::ContextDiff),
    ("contextDiffRemoveLine", Skipped::ContextDiff),
    ("contextDiffSingleSpace", Skipped::ContextDiff),
    ("contextNoNewlineAtEof", Skipped::ContextDiff),
    ("partialApply", Skipped::GenericFuzz),
];

// `ApplyPatchTest`: the case, `skipTopDirs` and the status it expects.
const CASES: [(&str, usize, ApplyStatus); 46] = {
    use ApplyStatus::*;
    [
        ("addLine", 1, Success),
        ("addLastLine", 1, Success),
        ("modifyLine", 1, Success),
        ("overlappingContext", 1, Success),
        ("addFile", 1, Success),
        ("removeFile", 1, Success),
        ("matchByContext", 1, Success),
        ("multiFile", 1, Success),
        ("contextDiff", 1, Success),
        ("contextDiffAddLine", 1, Success),
        ("contextDiffRemoveLine", 1, Success),
        ("contextDiffMultiFile", 1, Success),
        ("emptyLine", 1, Success),
        ("reversedNames", 0, Success),
        ("alreadyApplied", 1, AlreadyApplied),
        ("partialApply", 1, Partial),
        ("contextDiffSingleSpace", 1, Success),
        ("contextDiffSingleSpaceTrimmed", 1, Success),
        ("noNewlineAtEof", 1, Success),
        ("contextNoNewlineAtEof", 1, Success),
        ("renameFile", 1, Success),
        ("renameFileGitStyle", 1, Success),
        ("renameFileGitStyleWithWhitespaces", 1, Success),
        ("renameDir", 1, Success),
        ("deleteLastLineWithLineBreak", 1, Success),
        ("deleteLineContentWithoutLineBreak", 1, Success),
        ("deleteLastLineWithoutLineBreak", 1, Success),
        ("modifyFileNoHunkAtEOF", 1, Success),
        ("modifyFileRemoveLastEmptyLine", 1, Success),
        ("modifyFileAddLastEmptyLine", 1, Success),
        ("modifyFileLastLine", 1, Success),
        ("modifyFileKeepLastEmptyLine", 1, Success),
        ("moveFile", 1, Success),
        ("moveFileWithoutRename", 1, Success),
        ("moveAndRenameWithNameConflicts", 1, Success),
        ("incorrectAlreadyAppliedDetection", 1, Success),
        ("omittedChunkSize", 1, Success),
        ("wrongFileStartUnified", 1, Success),
        ("addFileWithGitVersion", 1, Success),
        ("modifyLineWithGitVersion", 1, Success),
        ("addFileWithoutNewlineAtEOF", 1, Success),
        ("fileWithGitStyleCyrillicPaths", 0, Success),
        ("renameFileGitStyleWithCyrillic", 0, Success),
        (
            "renameWithModificationWithGitStyleCyrillicPaths",
            0,
            Success,
        ),
        ("fileWithGitStylePathsWithSpaces", 0, Success),
        ("addEmptyFileWithWhitespaces", 1, Success),
    ]
};

#[test]
fn applies_intellij_fixtures_with_their_status() {
    let cases_on_disk: BTreeSet<String> = fs::read_dir(fixtures("applyPatch"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    let named: BTreeSet<String> = CASES.iter().map(|case| case.0.to_string()).collect();
    assert_eq!(cases_on_disk, named);

    for (name, skip, expected) in CASES {
        let dir = fixtures("applyPatch").join(name);
        let rename: &dyn Fn(&str) -> String = if name.contains("Cyrillic") {
            &cyrillic
        } else {
            &|name: &str| name.to_string()
        };
        let harness = RepoHarness::new();
        copy_tree(&dir.join("before"), harness.path(), rename);
        let before = tree(harness.path());
        if !before.is_empty() {
            harness.git(&["add", "-A"]);
            harness.git(&["commit", "-q", "-m", "before"]);
        }
        let text = fs::read(dir.join("apply.patch")).unwrap();
        let skipped = SKIPPED.iter().find(|(case, _)| *case == name);

        if let Some((_, Skipped::ContextDiff)) = skipped {
            let error = patch::read(&text).unwrap_err();
            assert!(
                matches!(&error, PatchError::Syntax { message, .. } if message == "Context diffs are not supported"),
                "{name}: {error:?}"
            );
            continue;
        }

        let patch = patch::read(&text).unwrap_or_else(|error| panic!("{name}: {error:?}"));
        let mapping = stripping(skip, patch.files.len());
        let (selected, hashes) = planned_files(harness.path(), &patch, &mapping);
        let result = harness
            .run(move |repo| apply(repo, &patch, &mapping, &selected, &[], &hashes))
            .unwrap();

        if let Some((_, Skipped::GenericFuzz)) = skipped {
            assert_eq!(result.status, ApplyStatus::Failure, "{name}");
            assert_eq!(tree(harness.path()), before, "{name}");
            continue;
        }

        assert_eq!(result.status, expected, "{name}: {result:?}");
        assert!(result.conflicts.is_empty(), "{name}");
        let mut after = BTreeMap::new();
        for (file, bytes) in tree(&dir.join("after")) {
            // `testRenameDir` leaves out the `empty` file that keeps the old folder.
            if Path::new(&file).file_stem().unwrap() != "empty" {
                after.insert(rename(&file), bytes);
            }
        }
        assert_eq!(tree(harness.path()), after, "{name}");
    }
}

#[test]
fn a_hunk_moved_by_three_lines_applies_at_the_offset() {
    let harness = RepoHarness::new();
    let original = numbered(1..=20);
    harness.repo.commit("a.txt", original.as_bytes(), "first");
    // Three lines added at the top move the hunk's lines 10 to 12 to 13 to 15.
    let local = format!("new 1\nnew 2\nnew 3\n{original}");
    harness.write("a.txt", &local);

    let result = apply_all(
        &harness,
        "--- a/a.txt\n+++ b/a.txt\n@@ -10,3 +10,3 @@\n line 10\n-line 11\n+eleven\n line 12\n",
    );

    assert_eq!(result.status, ApplyStatus::Success);
    assert_eq!(
        harness.read("a.txt"),
        local.replace("line 11\n", "eleven\n")
    );
}

#[test]
fn two_hunks_keep_their_order_at_offsets() {
    let harness = RepoHarness::new();
    // `same` appears twice: the second hunk must land after the first, not on the first match.
    let local = "x\nx\nsame\nA\nsame\nB\nz\nz\nz\n";
    harness.repo.commit("a.txt", local.as_bytes(), "first");
    let text = "--- a/a.txt\n+++ b/a.txt\n\
                @@ -1,2 +1,2 @@\n same\n-A\n+one\n\
                @@ -3,2 +3,2 @@\n same\n-B\n+two\n";

    let result = apply_all(&harness, text);

    assert_eq!(result.status, ApplyStatus::Success);
    assert_eq!(
        harness.read("a.txt"),
        "x\nx\nsame\none\nsame\ntwo\nz\nz\nz\n"
    );

    // A hunk that would only fit before the previous one is not placed there.
    let harness = RepoHarness::new();
    harness.repo.commit("a.txt", b"p\nq\nr\n", "first");
    let text = "--- a/a.txt\n+++ b/a.txt\n\
                @@ -3,1 +3,1 @@\n-r\n+R\n\
                @@ -4,1 +4,1 @@\n-p\n+P\n";
    let result = apply_all(&harness, text);
    assert_eq!(result.status, ApplyStatus::Failure);
    assert_eq!(result.failed, ["a.txt"]);
    assert_eq!(harness.read("a.txt"), "p\nq\nr\n");
}

#[test]
fn whole_file_already_applied() {
    let harness = RepoHarness::new();
    harness.repo.commit("a.txt", b"one\nTWO\nthree\n", "first");
    harness.repo.commit("b.txt", b"x\ny\n", "second");
    let text = "--- a/a.txt\n+++ b/a.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n\
                --- a/b.txt\n+++ b/b.txt\n@@ -1,2 +1,2 @@\n x\n-y\n+Y\n";

    let result = apply_all(&harness, text);

    // SUCCESS and ALREADY_APPLIED give PARTIAL.
    assert_eq!(result.status, ApplyStatus::Partial);
    assert_eq!(harness.read("a.txt"), "one\nTWO\nthree\n");
    assert_eq!(harness.read("b.txt"), "x\nY\n");

    let again = apply_all(
        &harness,
        "--- a/a.txt\n+++ b/a.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n",
    );
    assert_eq!(again.status, ApplyStatus::AlreadyApplied);
    let plan = plan(
        harness.path(),
        &read_patch("--- a/a.txt\n+++ b/a.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n"),
        &PathMapping::default(),
    );
    assert_eq!(plan.files[0].status, Status::AlreadyApplied);
}

// A shelf patch: the stored base (`<+>`) is the text the hunks were made from.
fn with_stored_base(base: &str, hunks: &str) -> String {
    let mut text = String::from("Index: a.txt\nIDEA additional info:\nSubsystem: com.intellij.openapi.diff.impl.patch.BaseRevisionTextPatchEP\n<+>");
    text.push_str(&base.replace('\n', "\\n"));
    text.push_str("\n===================================================================\n");
    text.push_str("--- a/a.txt\n+++ b/a.txt\n");
    text.push_str(hunks);
    text
}

#[test]
fn with_a_base_try_resolve_writes_the_merge() {
    let harness = RepoHarness::new();
    let base = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n";
    harness.repo.commit("a.txt", base.as_bytes(), "first");
    // The local file changed its first and last lines: the hunk's context no longer matches.
    let local = base.replace("one\n", "ONE\n").replace("three\n", "THREE\n");
    harness.write("a.txt", &local);
    let text = with_stored_base(base, "@@ -1,4 +1,4 @@\n one\n two\n three\n-four\n+FOUR\n");

    let plan = plan(harness.path(), &read_patch(&text), &PathMapping::default());
    assert_eq!(plan.files[0].status, Status::Applicable);
    assert!(plan.files[0].base);
    let result = apply_all(&harness, &text);

    assert_eq!(result.status, ApplyStatus::Success, "{result:?}");
    assert!(result.conflicts.is_empty());
    assert_eq!(harness.read("a.txt"), local.replace("four\n", "FOUR\n"));
}

#[test]
fn with_a_base_and_a_real_conflict_returns_a_conflict() {
    let harness = RepoHarness::new();
    let base = "one\ntwo\nthree\n";
    harness.repo.commit("a.txt", base.as_bytes(), "first");
    harness.write("a.txt", "one\nlocal\nthree\n");
    let text = with_stored_base(base, "@@ -1,3 +1,3 @@\n one\n-two\n+patched\n three\n");

    let result = apply_all(&harness, &text);

    assert_eq!(result.conflicts.len(), 1, "{result:?}");
    let conflict = &result.conflicts[0];
    assert_eq!(conflict.path, "a.txt");
    assert_eq!(conflict.base, base);
    assert_eq!(conflict.local, "one\nlocal\nthree\n");
    assert_eq!(conflict.patched, "one\npatched\nthree\n");
    assert_eq!(conflict.local_hash, content_hash(b"one\nlocal\nthree\n"));
    assert!(result.failed.is_empty());
    assert_eq!(harness.read("a.txt"), "one\nlocal\nthree\n");

    // A non-git patch takes its base from `(revision h)`.
    let harness = RepoHarness::new();
    let first = harness.repo.commit("a.txt", base.as_bytes(), "first");
    harness.write("a.txt", "one\nlocal\nthree\n");
    let text = format!(
        "Index: a.txt\n===================================================================\n\
         --- a.txt\t(revision {first})\n+++ a.txt\t(date 1)\n\
         @@ -1,3 +1,3 @@\n one\n-two\n+patched\n three\n"
    );
    let result = apply_all(&harness, &text);
    assert_eq!(result.conflicts.len(), 1, "{result:?}");
    assert_eq!(result.conflicts[0].base, base);
}

// `executePatchGroup` starts from no status once the pre-check passed: with only conflicts,
// the status is what their merge windows answer, so it comes as SKIP, which `and` leaves to
// them (SKIP with SUCCESS is SUCCESS, with SKIP is SKIP).
#[test]
fn with_only_conflicts_the_status_is_left_to_their_outcomes() {
    let harness = RepoHarness::new();
    let base = "one\ntwo\nthree\n";
    harness.repo.commit("a.txt", base.as_bytes(), "first");
    harness.write("a.txt", "one\nlocal\nthree\n");
    let text = with_stored_base(base, "@@ -1,3 +1,3 @@\n one\n-two\n+patched\n three\n");

    let result = apply_all(&harness, &text);

    assert_eq!(result.conflicts.len(), 1, "{result:?}");
    assert_eq!(result.status, ApplyStatus::Skip);
    let skipped = ApplyStatus::and(Some(result.status), Some(ApplyStatus::Skip));
    assert_eq!(skipped, Some(ApplyStatus::Skip));
    let resolved = ApplyStatus::and(Some(result.status), Some(ApplyStatus::Success));
    assert_eq!(resolved, Some(ApplyStatus::Success));
}

#[test]
fn without_a_base_the_file_fails() {
    let harness = RepoHarness::new();
    harness
        .repo
        .commit("a.txt", b"one\nlocal\nthree\n", "first");
    harness.repo.commit("b.txt", b"x\n", "second");
    let text = "--- a/a.txt\n+++ b/a.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+patched\n three\n\
                --- a/b.txt\n+++ b/b.txt\n@@ -1 +1 @@\n-x\n+y\n";

    let plan = plan(harness.path(), &read_patch(text), &PathMapping::default());
    assert_eq!(plan.files[0].status, Status::MissingBase);
    let result = apply_all(&harness, text);

    assert_eq!(result.status, ApplyStatus::Failure);
    assert_eq!(result.failed, ["a.txt"]);
    assert!(result.conflicts.is_empty());
    assert_eq!(harness.read("a.txt"), "one\nlocal\nthree\n");
    assert_eq!(harness.read("b.txt"), "y\n");
}

#[test]
fn new_file_that_exists_is_skipped_unless_overwrite() {
    let harness = RepoHarness::new();
    harness.repo.commit("other.txt", b"o\n", "first");
    harness.write("new.txt", "mine\n");
    let text = "--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+theirs\n";

    let plan = plan(harness.path(), &read_patch(text), &PathMapping::default());
    assert_eq!(plan.files[0].status, Status::Exists);
    assert_eq!(plan.files[0].kind, ChangeKind::Added);
    let skipped = apply_all(&harness, text);
    // IntelliJ counts a declined overwrite as skipped: all skipped gives PARTIAL there (the
    // pre-check compares the skipped count with what is left, not with the total).
    assert_eq!(skipped.status, ApplyStatus::Partial);
    assert!(skipped.failed.is_empty());
    assert_eq!(harness.read("new.txt"), "mine\n");

    let written = apply_with(&harness, text, &PathMapping::default(), &["new.txt"]);
    assert_eq!(written.status, ApplyStatus::Success);
    assert_eq!(harness.read("new.txt"), "theirs\n");
    // Added to git silently.
    assert_eq!(
        harness.git(&["diff", "--cached", "--name-only"]),
        "new.txt\n"
    );
}

#[test]
fn deleting_a_missing_file_counts_as_applied() {
    let harness = RepoHarness::new();
    harness.repo.commit("a.txt", b"one\n", "first");
    let deletion = "--- a/gone.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-old\n";
    let modification = "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-one\n+two\n";

    let result = apply_all(&harness, &format!("{deletion}{modification}"));

    assert!(result.failed.is_empty(), "{result:?}");
    assert_eq!(result.status, ApplyStatus::Partial);
    assert_eq!(harness.read("a.txt"), "two\n");

    // Alone, as IntelliJ: everything skipped is PARTIAL.
    let alone = apply_all(&harness, deletion);
    assert!(alone.failed.is_empty());
    assert_eq!(alone.status, ApplyStatus::Partial);

    // An existing file is deleted, and removed from git.
    let result = apply_all(
        &harness,
        "--- a/a.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-two\n",
    );
    assert_eq!(result.status, ApplyStatus::Success);
    assert!(!harness.path().join("a.txt").exists());
    assert_eq!(
        harness.git(&["diff", "--cached", "--name-status"]),
        "D\ta.txt\n"
    );
}

// A patch with a binary entry, as git writes it.
fn binary_patch(harness: &RepoHarness, before: &[u8], after: &[u8]) -> Vec<u8> {
    let blob = |bytes: &[u8]| {
        let path = harness.path().join(".blob");
        fs::write(&path, bytes).unwrap();
        let hash = harness
            .git(&["hash-object", "-w", ".blob"])
            .trim()
            .to_string();
        fs::remove_file(path).unwrap();
        hash
    };
    let (old, new) = (blob(before), blob(after));
    let output = std::process::Command::new("git")
        .current_dir(harness.path())
        .args(["diff", "--binary", "--full-index", &old, &new])
        .output()
        .unwrap();
    String::from_utf8(output.stdout)
        .unwrap()
        .replace(&format!("a/{old}"), "a/image.png")
        .replace(&format!("b/{new}"), "b/image.png")
        .into_bytes()
}

#[test]
fn binary_entry_applies_through_git() {
    let harness = RepoHarness::new();
    let before: Vec<u8> = (0u8..=255).cycle().take(2000).collect();
    let mut after = before.clone();
    after[10] = 0;
    after.extend_from_slice(b"\0tail");
    harness.repo.commit("image.png", &before, "first");
    let text = binary_patch(&harness, &before, &after);
    let patch = patch::read(&text).unwrap();
    assert!(matches!(patch.files[0], FilePatch::Binary(_)));

    let plan = plan(harness.path(), &patch, &PathMapping::default());
    assert_eq!(plan.files[0].status, Status::Applicable);
    assert!(plan.files[0].binary);
    let selected = vec!["image.png".to_string()];
    let applied = patch.clone();
    let result = harness
        .run(move |repo| {
            apply(
                repo,
                &applied,
                &PathMapping::default(),
                &selected,
                &[],
                &none(),
            )
        })
        .unwrap();

    assert_eq!(result.status, ApplyStatus::Success, "{result:?}");
    assert_eq!(fs::read(harness.path().join("image.png")).unwrap(), after);
    assert!(harness.ran("apply --whitespace=nowarn --binary"));

    // Applied again: git applies it reversed, so it is already there.
    let plan = super::plan(harness.path(), &patch, &PathMapping::default());
    assert_eq!(plan.files[0].status, Status::AlreadyApplied);
    let selected = vec!["image.png".to_string()];
    let again = harness
        .run(move |repo| {
            apply(
                repo,
                &patch,
                &PathMapping::default(),
                &selected,
                &[],
                &none(),
            )
        })
        .unwrap();
    assert_eq!(again.status, ApplyStatus::AlreadyApplied);
    assert_eq!(fs::read(harness.path().join("image.png")).unwrap(), after);
}

#[test]
fn a_snapshot_is_taken_before_any_write_and_rollback_restores_it() {
    let harness = RepoHarness::new();
    harness.repo.commit("a.txt", b"one\n", "first");
    harness.repo.commit("gone.txt", b"bye\n", "second");
    harness.repo.commit("old/moved.txt", b"m\n", "third");
    harness.write("a.txt", "one\n");
    let text = "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-one\n+two\n\
                --- a/gone.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\n\
                --- /dev/null\n+++ b/dir/new.txt\n@@ -0,0 +1 @@\n+new\n\
                diff --git a/old/moved.txt b/new/moved.txt\nsimilarity index 100%\n\
                rename from old/moved.txt\nrename to new/moved.txt\n";
    let before = tree(harness.path());
    let status_before = harness.git(&["status", "--porcelain"]);
    assert_eq!(harness.snapshots(), 0);

    let result = apply_all(&harness, text);

    assert_eq!(result.status, ApplyStatus::Success, "{result:?}");
    assert_eq!(harness.snapshots(), 1);
    let saved: BTreeSet<String> = harness.saved_paths().into_iter().collect();
    let expected: BTreeSet<String> = [
        "a.txt",
        "gone.txt",
        "dir/new.txt",
        "old/moved.txt",
        "new/moved.txt",
    ]
    .iter()
    .map(|path| path.to_string())
    .collect();
    assert_eq!(saved, expected);
    assert_eq!(harness.read("a.txt"), "two\n");
    assert_eq!(harness.read("new/moved.txt"), "m\n");
    assert!(!harness.path().join("old/moved.txt").exists());

    let (data, repo, id) = (
        harness.data.path().to_path_buf(),
        harness.path().to_path_buf(),
        result.snapshot.clone().unwrap(),
    );
    rollback(&data, &repo, &id).unwrap();

    assert_eq!(tree(harness.path()), before);
    assert_eq!(harness.git(&["status", "--porcelain"]), status_before);
}

#[test]
fn a_file_changed_after_reading_is_not_overwritten() {
    let harness = RepoHarness::new();
    harness.repo.commit("a.txt", b"one\n", "first");
    harness.repo.commit("b.txt", b"x\n", "second");
    let patch = read_patch(
        "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-one\n+two\n\
         --- a/b.txt\n+++ b/b.txt\n@@ -1 +1 @@\n-x\n+y\n",
    );
    let root = harness.path().to_path_buf();
    let selected = vec!["a.txt".to_string(), "b.txt".to_string()];

    let result = harness
        .run(move |repo| {
            apply_watching(
                repo,
                &patch,
                &PathMapping::default(),
                &Files {
                    selected: &selected,
                    overwrite: &[],
                    expected: &none(),
                },
                &mut |moment, path| {
                    if moment == Moment::Write && path == "a.txt" {
                        fs::write(root.join("a.txt"), "devin\n").unwrap();
                    }
                },
            )
        })
        .unwrap();

    assert_eq!(result.status, ApplyStatus::Failure);
    assert_eq!(result.failed, ["a.txt"]);
    assert_eq!(result.messages, [changed_while_applying("a.txt")]);
    assert_eq!(harness.read("a.txt"), "devin\n");
    assert_eq!(harness.read("b.txt"), "y\n");
}

#[test]
fn patch_conflict_resolve_refuses_when_the_local_hash_changed() {
    let harness = RepoHarness::new();
    let base = "one\ntwo\nthree\n";
    harness.repo.commit("a.txt", base.as_bytes(), "first");
    harness.write("a.txt", "one\nlocal\nthree\n");
    let text = with_stored_base(base, "@@ -1,3 +1,3 @@\n one\n-two\n+patched\n three\n");
    let result = apply_all(&harness, &text);
    let conflict = result.conflicts[0].clone();

    harness.write("a.txt", "one\ndevin\nthree\n");
    let refused = resolve_conflict(
        harness.path(),
        "a.txt",
        "one\nresolved\nthree\n",
        &conflict.local_hash,
        None,
        None,
    );
    assert!(matches!(refused, Err(GitError::Io(ref message)) if message == FILE_CHANGED));
    assert_eq!(harness.read("a.txt"), "one\ndevin\nthree\n");

    harness.write("a.txt", "one\nlocal\nthree\n");
    let hash = resolve_conflict(
        harness.path(),
        "a.txt",
        "one\nresolved\nthree\n",
        &conflict.local_hash,
        None,
        None,
    )
    .unwrap();
    assert_eq!(harness.read("a.txt"), "one\nresolved\nthree\n");
    assert_eq!(hash.hash, content_hash(b"one\nresolved\nthree\n"));
}

#[test]
fn a_conflict_on_a_moved_file_moves_it_when_resolved() {
    let harness = RepoHarness::new();
    let base = "one\ntwo\nthree\n";
    harness.repo.commit("old.txt", base.as_bytes(), "first");
    harness.write("old.txt", "one\nlocal\nthree\n");
    let text = format!(
        "Index: old.txt\nIDEA additional info:\nSubsystem: com.intellij.openapi.diff.impl.patch.BaseRevisionTextPatchEP\n<+>{}\n\
         ===================================================================\n\
         diff --git a/old.txt b/new.txt\nold mode 100644\nnew mode 100755\nrename from old.txt\nrename to new.txt\n\
         --- a/old.txt\n+++ b/new.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+patched\n three\n",
        base.replace('\n', "\\n")
    );
    let result = apply_all(&harness, &text);
    let conflict = result.conflicts[0].clone();
    assert_eq!(conflict.path, "old.txt");
    assert_eq!(conflict.moved_to.as_deref(), Some("new.txt"));
    assert_eq!(conflict.new_mode, Some(EXECUTABLE_FILE_MODE));
    assert!(harness.path().join("old.txt").exists());

    resolve_conflict(
        harness.path(),
        "old.txt",
        "one\nresolved\nthree\n",
        &conflict.local_hash,
        Some("new.txt"),
        Some(EXECUTABLE_FILE_MODE),
    )
    .unwrap();

    assert!(!harness.path().join("old.txt").exists());
    assert_eq!(harness.read("new.txt"), "one\nresolved\nthree\n");
    let mode = fs::metadata(harness.path().join("new.txt"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & OWNER_EXECUTE, OWNER_EXECUTE);
}

#[test]
fn untrusted_paths_and_modes_fail_their_file() {
    let harness = RepoHarness::new();
    harness.repo.commit("a.txt", b"one\n", "first");
    let outside = harness.path().parent().unwrap().join("outside.txt");
    let _ = fs::remove_file(&outside);
    let text = "--- /dev/null\n+++ b/../outside.txt\n@@ -0,0 +1 @@\n+x\n\
                --- /dev/null\n+++ b/.git/hooks/pre-commit\n@@ -0,0 +1 @@\n+x\n\
                diff --git a/odd.txt b/odd.txt\nnew file mode 100777\n--- /dev/null\n+++ b/odd.txt\n@@ -0,0 +1 @@\n+x\n\
                --- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-one\n+two\n";
    let patch = read_patch(text);

    let plan = plan(harness.path(), &patch, &PathMapping::default());
    let statuses: Vec<Status> = plan.files.iter().map(|file| file.status).collect();
    assert_eq!(
        statuses,
        [
            Status::Invalid,
            Status::Invalid,
            Status::Invalid,
            Status::Applicable
        ]
    );
    assert_eq!(
        plan.files[2].error.as_deref(),
        Some(unknown_mode("odd.txt", 0o100777).as_str())
    );
    let selected: Vec<String> = plan.files.iter().map(|file| file.path.clone()).collect();
    let result = harness
        .run(move |repo| {
            apply(
                repo,
                &patch,
                &PathMapping::default(),
                &selected,
                &[],
                &none(),
            )
        })
        .unwrap();

    assert_eq!(result.status, ApplyStatus::Failure);
    assert_eq!(result.failed.len(), 3);
    assert!(!outside.exists());
    assert!(!harness.path().join(".git/hooks/pre-commit").exists());
    assert!(!harness.path().join("odd.txt").exists());
    assert_eq!(harness.read("a.txt"), "two\n");
}

#[test]
fn strip_and_base_change_the_targets() {
    let harness = RepoHarness::new();
    harness.repo.commit("sub/dir/a.txt", b"one\n", "first");
    let patch = read_patch("--- a/x/y/a.txt\n+++ b/x/y/a.txt\n@@ -1 +1 @@\n-one\n+two\n");

    let plain = plan(harness.path(), &patch, &PathMapping::default());
    assert_eq!(plain.files[0].path, "x/y/a.txt");
    assert_eq!(plain.files[0].status, Status::Missing);
    assert_eq!(plain.files[0].max_strip, 2);

    // Strip past the maximum keeps the file name.
    let all = plan(harness.path(), &patch, &stripping(9, 1));
    assert_eq!(all.files[0].path, "a.txt");
    assert_eq!(all.files[0].strip, 2);

    let mapping = PathMapping {
        files: vec![FileMapping {
            strip: 1,
            base: "sub".to_string(),
        }],
    };
    let mapped = plan(harness.path(), &patch, &mapping);
    assert_eq!(mapped.files[0].path, "sub/y/a.txt");
    let mapping = PathMapping {
        files: vec![FileMapping {
            strip: 2,
            base: "sub/dir".to_string(),
        }],
    };
    let mapped = plan(harness.path(), &patch, &mapping);
    assert_eq!(mapped.files[0].path, "sub/dir/a.txt");
    assert_eq!(mapped.files[0].original, "x/y/a.txt");
    assert_eq!(mapped.files[0].status, Status::Applicable);

    let result = apply_with(
        &harness,
        "--- a/x/y/a.txt\n+++ b/x/y/a.txt\n@@ -1 +1 @@\n-one\n+two\n",
        &mapping,
        &[],
    );
    assert_eq!(result.status, ApplyStatus::Success);
    assert_eq!(harness.read("sub/dir/a.txt"), "two\n");
}

#[test]
fn crlf_files_keep_their_line_separator() {
    let harness = RepoHarness::new();
    harness
        .repo
        .commit("a.txt", b"one\r\ntwo\r\nthree\r\n", "first");

    let result = apply_all(
        &harness,
        "--- a/a.txt\n+++ b/a.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n",
    );

    assert_eq!(result.status, ApplyStatus::Success);
    assert_eq!(
        fs::read(harness.path().join("a.txt")).unwrap(),
        b"one\r\nTWO\r\nthree\r\n"
    );
}

#[test]
fn a_repository_switch_is_refused_before_any_write() {
    let harness = RepoHarness::new();
    harness.repo.commit("a.txt", b"one\n", "first");
    let source = PatchInput::Text {
        text: "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-one\n+two\n".to_string(),
    };
    let selected = vec!["a.txt".to_string()];

    let refused = harness.run(move |repo| {
        apply_input(
            repo,
            &source,
            &PathMapping::default(),
            &ApplyChoice {
                selected,
                ..ApplyChoice::default()
            },
            "0000",
        )
    });

    assert!(matches!(refused, Err(GitError::Io(_))));
    assert_eq!(harness.read("a.txt"), "one\n");
    assert_eq!(harness.snapshots(), 0);
}

#[test]
fn plan_of_an_unreadable_patch_says_why() {
    let harness = RepoHarness::new();
    let source = PatchInput::Text {
        text: "*** a.txt\n--- a.txt\n***************\n*** 1 ****\n".to_string(),
    };
    let plan = plan_input(
        harness.path(),
        harness.data.path(),
        &source,
        &PathMapping::default(),
    )
    .unwrap();
    assert_eq!(
        plan.read_error.as_deref(),
        Some("Context diffs are not supported")
    );
    assert!(plan.files.is_empty());

    let file = harness.path().join("x.patch");
    fs::write(
        &file,
        "Subject: [PATCH] Fix it\n---\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-a\n+b\n",
    )
    .unwrap();
    let source = PatchInput::File {
        path: file.to_string_lossy().into_owned(),
    };
    let plan = plan_input(
        harness.path(),
        harness.data.path(),
        &source,
        &PathMapping::default(),
    )
    .unwrap();
    assert_eq!(plan.subject.as_deref(), Some("Fix it"));
    assert_eq!(plan.files[0].status, Status::Missing);
    assert_eq!(plan.files[0].kind, ChangeKind::Modified);
}

#[test]
fn a_link_to_git_is_never_followed() {
    let harness = RepoHarness::new();
    harness.repo.commit("a.txt", b"one\n", "first");
    std::os::unix::fs::symlink(".git", harness.path().join("x")).unwrap();
    harness.git(&["add", "x"]);
    harness.git(&["commit", "-q", "-m", "link"]);
    let config = fs::read(harness.path().join(".git/config")).unwrap();
    let description = fs::read(harness.path().join(".git/description")).unwrap();
    let first = String::from_utf8(config.clone()).unwrap();
    let first = first.lines().next().unwrap().to_string();
    let text = format!(
        "--- a/x/config\n+++ b/x/config\n@@ -1 +1 @@\n-{first}\n+[evil]\n\
         --- a/x/description\n+++ /dev/null\n@@ -1 +0,0 @@\n-whatever\n"
    );
    let patch = read_patch(&text);

    let plan = plan(harness.path(), &patch, &PathMapping::default());
    let statuses: Vec<Status> = plan.files.iter().map(|file| file.status).collect();
    assert_eq!(statuses, [Status::Invalid, Status::Invalid], "{plan:?}");
    let selected = vec!["x/config".to_string(), "x/description".to_string()];
    let result = harness
        .run(move |repo| {
            apply(
                repo,
                &patch,
                &PathMapping::default(),
                &selected,
                &[],
                &none(),
            )
        })
        .unwrap();

    assert_eq!(result.status, ApplyStatus::Failure);
    assert_eq!(result.failed, ["x/config", "x/description"]);
    assert_eq!(
        fs::read(harness.path().join(".git/config")).unwrap(),
        config
    );
    assert_eq!(
        fs::read(harness.path().join(".git/description")).unwrap(),
        description
    );

    // Nor through the merge window's result.
    let hash = content_hash(&config);
    assert!(resolve_conflict(harness.path(), "x/config", "[evil]\n", &hash, None, None).is_err());
    assert_eq!(
        fs::read(harness.path().join(".git/config")).unwrap(),
        config
    );
    let hash = content_hash(b"one\n");
    let moved = resolve_conflict(
        harness.path(),
        "a.txt",
        "one\n",
        &hash,
        Some("x/hooks/pre-commit"),
        None,
    );
    assert!(moved.is_err());
    assert!(!harness.path().join(".git/hooks/pre-commit").exists());
    assert_eq!(harness.read("a.txt"), "one\n");
}

// A committed link into `.git` is the user's (the shared `check_path` accepts it), but the
// applier never acts through a final link.
#[test]
fn a_patch_on_a_link_into_git_fails() {
    let harness = RepoHarness::new();
    std::os::unix::fs::symlink(".git/hooks", harness.path().join("hooks")).unwrap();
    harness.git(&["add", "hooks"]);
    harness.git(&["commit", "-q", "-m", "link"]);
    let hooks = tree(&harness.path().join(".git/hooks"));
    let patch = read_patch(
        "--- a/hooks\n+++ b/hooks\n@@ -1 +1 @@\n-.git/hooks\n+x\n\
         --- /dev/null\n+++ b/hooks/pre-commit\n@@ -0,0 +1 @@\n+evil\n",
    );

    let plan = plan(harness.path(), &patch, &PathMapping::default());
    let statuses: Vec<Status> = plan.files.iter().map(|file| file.status).collect();
    assert_eq!(statuses, [Status::Invalid, Status::Invalid], "{plan:?}");
    let (selected, expected) = planned_files(harness.path(), &patch, &PathMapping::default());
    let result = harness
        .run(move |repo| {
            apply(
                repo,
                &patch,
                &PathMapping::default(),
                &selected,
                &[],
                &expected,
            )
        })
        .unwrap();

    assert_eq!(result.status, ApplyStatus::Failure);
    assert_eq!(result.failed.len(), 2);
    assert_eq!(tree(&harness.path().join(".git/hooks")), hooks);
    assert!(harness
        .path()
        .join("hooks")
        .symlink_metadata()
        .unwrap()
        .file_type()
        .is_symlink());
    let hash = content_hash(b".git/hooks");
    assert!(resolve_conflict(harness.path(), "hooks", "x\n", &hash, None, None).is_err());
    assert!(harness
        .path()
        .join("hooks")
        .symlink_metadata()
        .unwrap()
        .file_type()
        .is_symlink());
}

#[test]
fn set_mode_never_follows_a_link() {
    let harness = RepoHarness::new();
    let target = harness.path().join("target.sh");
    fs::write(&target, "x").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
    std::os::unix::fs::symlink("target.sh", harness.path().join("link")).unwrap();

    assert!(set_mode(harness.path(), "link", Some(EXECUTABLE_FILE_MODE)).is_some());

    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o644
    );
}

#[test]
fn a_mode_error_after_the_merge_is_written_still_moves() {
    let harness = RepoHarness::new();
    harness.repo.commit("old.txt", b"one\n", "first");
    let hash = content_hash(b"one\n");

    // Another program puts a link where the merge was just written: the mode is not set
    // through it, and the move still happens.
    let resolved = resolve_conflict_with(
        harness.path(),
        "old.txt",
        "merged\n",
        &hash,
        Some("new.txt"),
        Some(EXECUTABLE_FILE_MODE),
        &mut |full| {
            fs::remove_file(full).unwrap();
            std::os::unix::fs::symlink("elsewhere.txt", full).unwrap();
        },
    )
    .unwrap();

    assert_eq!(resolved.hash, content_hash(b"merged\n"));
    assert_eq!(resolved.messages.len(), 1, "{resolved:?}");
    assert!(resolved.messages[0].starts_with("Cannot change the mode of old.txt"));
    assert!(harness.path().join("old.txt").symlink_metadata().is_err());
    assert!(harness.path().join("new.txt").symlink_metadata().is_ok());
}

#[test]
fn the_aside_name_never_replaces_a_file() {
    let harness = RepoHarness::new();
    let full = harness.path().join("image.png");
    fs::write(&full, b"mine").unwrap();
    let taken = aside_numbered(&full, 0);
    fs::write(&taken, b"someone else's").unwrap();

    let aside = set_aside(&full).unwrap();

    assert_ne!(aside, taken);
    assert_eq!(fs::read(&taken).unwrap(), b"someone else's");
    assert_eq!(fs::read(&aside).unwrap(), b"mine");
    assert!(!full.exists());
}

#[test]
fn a_file_edited_after_the_plan_is_not_overwritten() {
    let harness = RepoHarness::new();
    harness.repo.commit("a.txt", b"one\n", "first");
    harness.repo.commit("b.txt", b"x\n", "second");
    let patch = read_patch(
        "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-one\n+two\n\
         --- a/b.txt\n+++ b/b.txt\n@@ -1 +1 @@\n-x\n+y\n\
         --- /dev/null\n+++ b/n.txt\n@@ -0,0 +1 @@\n+new\n",
    );
    let (selected, expected) = planned_files(harness.path(), &patch, &PathMapping::default());
    assert_eq!(expected["a.txt"], content_hash(b"one\n"));
    assert_eq!(expected["n.txt"], ABSENT);

    // Still applicable, but not what the dialog showed.
    harness.write("a.txt", "one\nedited\n");
    harness.write("n.txt", "mine\n");
    let overwrite = vec!["n.txt".to_string()];
    let result = harness
        .run(move |repo| {
            apply(
                repo,
                &patch,
                &PathMapping::default(),
                &selected,
                &overwrite,
                &expected,
            )
        })
        .unwrap();

    assert_eq!(result.status, ApplyStatus::Failure);
    assert_eq!(result.failed, ["a.txt", "n.txt"]);
    assert_eq!(
        result.messages,
        [
            changed_while_applying("a.txt"),
            changed_while_applying("n.txt")
        ]
    );
    assert_eq!(harness.read("a.txt"), "one\nedited\n");
    assert_eq!(harness.read("n.txt"), "mine\n");
    assert_eq!(harness.read("b.txt"), "y\n");
}

// A new binary file over an existing one, with a section git refuses.
const CORRUPT_BINARY: &str = "diff --git a/image.png b/image.png\nnew file mode 100644\n\
    index 0000000000000000000000000000000000000000..1111111111111111111111111111111111111111\n\
    GIT binary patch\nliteral 5\nMcorrupt\n\nliteral 0\nHcmV?d00001\n\n";

#[test]
fn a_failed_binary_overwrite_puts_the_file_back_with_its_mode() {
    let harness = RepoHarness::new();
    harness.repo.commit("a.txt", b"one\n", "first");
    let image = harness.path().join("image.png");
    fs::write(&image, b"\0mine\0").unwrap();
    fs::set_permissions(&image, fs::Permissions::from_mode(0o755)).unwrap();
    let patch = patch::read(CORRUPT_BINARY.as_bytes()).unwrap();
    let plan = plan(harness.path(), &patch, &PathMapping::default());
    assert_eq!(plan.files[0].status, Status::Exists);

    let (selected, expected) = planned_files(harness.path(), &patch, &PathMapping::default());
    let overwrite = selected.clone();
    let result = harness
        .run(move |repo| {
            apply(
                repo,
                &patch,
                &PathMapping::default(),
                &selected,
                &overwrite,
                &expected,
            )
        })
        .unwrap();

    assert_eq!(result.status, ApplyStatus::Failure);
    assert_eq!(result.failed, ["image.png"]);
    assert_eq!(result.messages.len(), 1, "{result:?}");
    assert_eq!(fs::read(&image).unwrap(), b"\0mine\0");
    let mode = fs::metadata(&image).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o755);
    assert_eq!(tree(harness.path()).len(), 2);
}

#[test]
fn a_file_that_cannot_be_put_back_names_the_copy() {
    let harness = RepoHarness::new();
    let aside = harness.path().join(".image.png.gone");

    let error = put_back(harness.path(), &aside, "image.png", "copy-1").unwrap_err();

    assert!(error.contains("image.png"), "{error}");
    assert!(error.contains("copy-1"), "{error}");
}

#[test]
fn rollback_gives_the_index_back() {
    let harness = RepoHarness::new();
    harness.repo.commit("s.txt", b"s\n", "first");
    harness.repo.commit("b.txt", b"1\n", "second");
    harness.repo.commit("c.txt", b"1\n", "third");
    // A staged deletion, a staged blob that differs from the disk, a stage equal to the disk.
    harness.git(&["rm", "-q", "s.txt"]);
    harness.write("b.txt", "2\n");
    harness.git(&["add", "b.txt"]);
    harness.write("b.txt", "3\n");
    harness.write("c.txt", "2\n");
    harness.git(&["add", "c.txt"]);
    let before = tree(harness.path());
    let status_before = harness.git(&["status", "--porcelain"]);
    let text = "--- /dev/null\n+++ b/s.txt\n@@ -0,0 +1 @@\n+new\n\
                --- a/b.txt\n+++ b/b.txt\n@@ -1 +1 @@\n-3\n+4\n\
                --- a/c.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-2\n";

    let result = apply_all(&harness, text);
    assert_eq!(result.status, ApplyStatus::Success, "{result:?}");
    assert_ne!(harness.git(&["status", "--porcelain"]), status_before);
    let (data, repo) = (harness.data.path(), harness.path());
    rollback(data, repo, &result.snapshot.unwrap()).unwrap();

    assert_eq!(tree(harness.path()), before);
    assert_eq!(harness.git(&["status", "--porcelain"]), status_before);
    assert_eq!(harness.git(&["show", ":b.txt"]), "2\n");
    assert_eq!(harness.git(&["show", ":c.txt"]), "2\n");
}

#[test]
fn nothing_to_write_takes_no_copy() {
    let harness = RepoHarness::new();
    harness.repo.commit("a.txt", b"one\n", "first");

    let skipped = apply_all(
        &harness,
        "--- a/gone.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-old\n",
    );
    let failed = apply_all(
        &harness,
        "--- a/gone.txt\n+++ b/gone.txt\n@@ -1 +1 @@\n-a\n+b\n",
    );

    assert_eq!(skipped.snapshot, None);
    assert_eq!(failed.snapshot, None);
    assert_eq!(failed.status, ApplyStatus::Failure);
    assert_eq!(harness.snapshots(), 0);
}

#[test]
fn a_binary_entry_never_makes_a_link() {
    let harness = RepoHarness::new();
    let before: Vec<u8> = (0u8..=255).collect();
    let mut after = before.clone();
    after[0] = 1;
    harness.repo.commit("image.png", &before, "first");
    let text = String::from_utf8(binary_patch(&harness, &before, &after)).unwrap();
    let (header, rest) = text.split_once('\n').unwrap();
    let text = format!("{header}\nold mode 100644\nnew mode 120000\n{rest}");
    let patch = patch::read(text.as_bytes()).unwrap();

    let plan = plan(harness.path(), &patch, &PathMapping::default());
    assert_eq!(plan.files[0].status, Status::Invalid);
    assert_eq!(
        plan.files[0].error.as_deref(),
        Some(binary_link("image.png").as_str())
    );
    let selected = vec!["image.png".to_string()];
    let result = harness
        .run(move |repo| {
            apply(
                repo,
                &patch,
                &PathMapping::default(),
                &selected,
                &[],
                &none(),
            )
        })
        .unwrap();
    assert_eq!(result.status, ApplyStatus::Failure);
    let meta = harness.path().join("image.png").symlink_metadata().unwrap();
    assert!(meta.is_file());
    assert_eq!(fs::read(harness.path().join("image.png")).unwrap(), before);
}

#[test]
fn a_file_changed_between_the_copy_and_the_read_fails() {
    let harness = RepoHarness::new();
    harness.repo.commit("a.txt", b"one\n", "first");
    let patch = read_patch("--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-one\n+two\n");
    let root = harness.path().to_path_buf();
    let selected = vec!["a.txt".to_string()];

    let result = harness
        .run(move |repo| {
            let files = Files {
                selected: &selected,
                overwrite: &[],
                expected: &none(),
            };
            apply_watching(
                repo,
                &patch,
                &PathMapping::default(),
                &files,
                &mut |moment, _| {
                    if moment == Moment::Read {
                        fs::write(root.join("a.txt"), "devin\n").unwrap();
                    }
                },
            )
        })
        .unwrap();

    assert_eq!(result.failed, ["a.txt"]);
    assert_eq!(result.messages, [changed_while_applying("a.txt")]);
    assert_eq!(harness.read("a.txt"), "devin\n");
    let copy = snapshots::stored(
        harness.data.path(),
        &result.snapshot.unwrap(),
        "a.txt",
        false,
    );
    assert_eq!(fs::read(copy.unwrap()).unwrap(), b"one\n");
}

#[test]
fn a_new_file_whose_hunks_cannot_be_built_is_not_applicable() {
    let harness = RepoHarness::new();
    let text = "diff --git a/n.txt b/n.txt\nnew file mode 100644\n--- /dev/null\n+++ b/n.txt\n\
                @@ -0,0 +1 @@\n+a\n@@ -5,1 +6,1 @@\n-x\n+y\n";

    let plan = plan(harness.path(), &read_patch(text), &PathMapping::default());

    assert_eq!(plan.files[0].kind, ChangeKind::Added);
    assert_eq!(plan.files[0].status, Status::MissingBase);
    let result = apply_all(&harness, text);
    assert_eq!(result.failed, ["n.txt"]);
    assert!(!harness.path().join("n.txt").exists());
}

// `Show Difference`: the file as it is and what the patch makes of it, with
// `ChangeDiffRequestProducer`'s titles; a file the patch cannot reach says why.
#[test]
fn show_difference_gives_the_local_and_the_patched_text() {
    let harness = RepoHarness::new();
    harness.repo.commit("a.txt", b"one\ntwo\n", "first");
    let text = "--- a/a.txt\n+++ b/a.txt\n@@ -1,2 +1,2 @@\n one\n-two\n+TWO\n\
                diff --git a/n.txt b/n.txt\nnew file mode 100644\n--- /dev/null\n+++ b/n.txt\n\
                @@ -0,0 +1 @@\n+new\n\
                --- a/gone.txt\n+++ b/gone.txt\n@@ -1 +1 @@\n-x\n+y\n";
    let source = PatchInput::Text {
        text: text.to_string(),
    };
    let mapping = PathMapping::default();

    let modified =
        preview_input(harness.path(), harness.data.path(), &source, &mapping, 0).unwrap();
    assert_eq!(modified.before, "one\ntwo\n");
    assert_eq!(modified.after, "one\nTWO\n");
    assert_eq!(modified.titles, ["Base version", "Patched version"]);
    let added = preview_input(harness.path(), harness.data.path(), &source, &mapping, 1).unwrap();
    assert_eq!((added.before.as_str(), added.after.as_str()), ("", "new\n"));
    let missing =
        preview_input(harness.path(), harness.data.path(), &source, &mapping, 2).unwrap_err();
    assert_eq!(
        ApiError::from(missing).message,
        "Cannot find base for 'gone.txt'"
    );
    assert!(preview_input(harness.path(), harness.data.path(), &source, &mapping, 3).is_err());
}

// `PatchDiffRequestFactory.createConflictDiffRequest`: a file that is not text says it is
// binary (`changes.error.can.t.show.diff.for.binary.file`, V:1049).
#[test]
fn show_difference_of_a_file_that_is_not_text_says_it_is_binary() {
    let harness = RepoHarness::new();
    let first = harness.repo.commit("a.txt", b"one\ntwo\nthree\n", "first");
    let local = b"\xff\xfe\none\nlocal\nthree\n";
    fs::write(harness.path().join("a.txt"), local).unwrap();
    let source = PatchInput::Text {
        text: format!(
            "Index: a.txt\n===================================================================\n\
             --- a.txt\t(revision {first})\n+++ a.txt\t(date 1)\n\
             @@ -1,3 +1,3 @@\n one\n-two\n+patched\n three\n"
        ),
    };

    let shown = preview_input(
        harness.path(),
        harness.data.path(),
        &source,
        &PathMapping::default(),
        0,
    );

    assert_eq!(
        ApiError::from(shown.unwrap_err()).message,
        "Cannot show diff for binary file 'a.txt'"
    );
}

#[test]
fn show_difference_follows_the_mapping() {
    let harness = RepoHarness::new();
    harness.repo.commit("src/a.txt", b"one\n", "first");
    let source = PatchInput::Text {
        text: "--- a/x/a.txt\n+++ b/x/a.txt\n@@ -1 +1 @@\n-one\n+uno\n".to_string(),
    };
    let mapping = PathMapping {
        files: vec![FileMapping {
            strip: 1,
            base: "src".to_string(),
        }],
    };

    let shown = preview_input(harness.path(), harness.data.path(), &source, &mapping, 0).unwrap();

    assert_eq!(
        (shown.before.as_str(), shown.after.as_str()),
        ("one\n", "uno\n")
    );
}
