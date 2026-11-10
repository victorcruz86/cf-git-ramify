// gpg as 241's `GitGpgConfigUtils` uses it: the secret keys that can sign, and the repository's
// signing config. git signs by itself from that config; rama only runs gpg to list the keys.
use std::env;
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Serialize;

use crate::branches;
use crate::git::{self, GitError};
use crate::task::TaskContext;

// Decision 3: where gpg is looked for after `PATH`, and the folders every git that rama starts
// gets at the end of its `PATH`.
pub const FOLDERS: [&str; 3] = [
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/usr/local/MacGPG2/bin",
];
const NAME: &str = "gpg";
// `readAvailableSecretKeys` (`GitGpgConfigUtils.kt:34`).
const LIST_ARGS: [&str; 5] = [
    "--list-secret-keys",
    "--with-colons",
    "--fixed-list-mode",
    "--batch",
    "--no-tty",
];
// Decision 4, rama's own text: a GPG key ID in `user.signingkey` breaks every SSH-signed commit.
const SSH_FORMAT: &str =
    "Commits are signed with SSH (gpg.format=ssh). Change gpg.format to use a GPG key.";
// `ENOENT`: what a program that is not there gives.
const NO_SUCH_FILE: i32 = 2;

// A primary key that can sign, with the first user ID gpg lists for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpgKey {
    pub id: String,
    pub user_id: Option<String>,
}

// The effective values, from any scope: `commit.gpgSign`, `user.signingkey` and whether
// `gpg.format` is `ssh`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SigningConfig {
    pub sign: bool,
    pub key: Option<String>,
    pub ssh: bool,
}

// Decision 3: the repository's `gpg.program`, else `gpg` on `PATH`, else the folders. Always an
// absolute path: rama never runs a program by name. `--path` expands `~` as git does when it reads
// `gpg.program` to sign. A config git cannot read is an error, never a quiet fallback.
pub fn program(repo: &Path) -> Result<Option<PathBuf>, GitError> {
    let configured = value(&quiet(repo), &["--path", "gpg.program"])?;
    Ok(resolve(
        repo,
        configured.as_deref(),
        env::var_os("PATH").as_deref(),
        &FOLDERS,
    ))
}

// A configured path is taken as it is (relative ones from the repository, where git runs gpg), and
// running it says whether it is there. A bare name is looked for as git looks for it.
fn resolve(
    repo: &Path,
    configured: Option<&str>,
    path: Option<&OsStr>,
    folders: &[&str],
) -> Option<PathBuf> {
    match configured.filter(|configured| !configured.is_empty()) {
        Some(configured) if configured.contains('/') => Some(repo.join(configured)),
        Some(name) => find(name, path, folders),
        None => find(NAME, path, folders),
    }
}

fn find(name: &str, path: Option<&OsStr>, folders: &[&str]) -> Option<PathBuf> {
    path.into_iter()
        .flat_map(env::split_paths)
        .chain(folders.iter().map(PathBuf::from))
        .filter(|folder| folder.is_absolute())
        .map(|folder| folder.join(name))
        .find(|candidate| candidate.is_file())
}

// Like 241 (`GitImpl.runBundledCommand`), the exit code is not looked at and only stdout counts: a
// gpg that fails lists nothing. No stdin and `--batch`: it never waits for anyone.
pub fn secret_keys(program: &Path) -> Result<Vec<GpgKey>, GitError> {
    let output = Command::new(program)
        .args(LIST_ARGS)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| cannot_run(program, &error))?;
    Ok(parse_secret_keys(&String::from_utf8_lossy(&output.stdout)))
}

// When no gpg was found: what running `gpg` by name would have said.
pub fn not_found() -> GitError {
    cannot_run(Path::new(NAME), &io::Error::from_raw_os_error(NO_SUCH_FILE))
}

// The message of 241's `ExecutionException`, with the system's reason.
fn cannot_run(program: &Path, error: &io::Error) -> GitError {
    GitError::Io(format!(
        "Cannot run program \"{}\": {error}",
        program.display()
    ))
}

// `parseSecretKeys` (`GitGpgConfigUtils.kt:42-80`), on gnupg's colon format: a `sec` record that is
// not disabled and can sign, and the first `uid` after it. A rejected `sec` takes no `uid`.
fn parse_secret_keys(output: &str) -> Vec<GpgKey> {
    let mut keys: Vec<GpgKey> = Vec::new();
    let mut last: Option<usize> = None;
    for line in output.lines() {
        let fields: Vec<&str> = line.split(':').collect();
        match fields[0] {
            "sec" => {
                last = match (fields.get(4), fields.get(11)) {
                    (Some(id), Some(capabilities)) if can_sign(capabilities) => {
                        keys.push(GpgKey {
                            id: id.to_string(),
                            user_id: None,
                        });
                        Some(keys.len() - 1)
                    }
                    _ => None,
                };
            }
            "uid" => {
                if let (Some(user_id), Some(index)) = (fields.get(9), last) {
                    keys[index].user_id = Some(unescape(user_id));
                }
                last = None;
            }
            _ => {}
        }
    }
    keys
}

fn can_sign(capabilities: &str) -> bool {
    !capabilities.contains('D') && (capabilities.contains('s') || capabilities.contains('S'))
}

// `StringUtil.unescapeAnsiStringCharacters`: C escapes, `\x` with two hex digits, `\u` with four,
// `\U` with eight and octal with three; a code becomes one UTF-16 unit, as Java's `(char)` cast.
// What is not an escape keeps its backslash or loses it as there.
fn unescape(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        let current = chars[index];
        index += 1;
        if current != '\\' {
            out.push(current);
            continue;
        }
        let Some(&escape) = chars.get(index) else {
            out.push('\\');
            break;
        };
        index += 1;
        let (count, radix, start) = match escape {
            '\'' | '"' | '?' | '\\' => {
                out.push(escape);
                continue;
            }
            'a' => {
                out.push('\u{7}');
                continue;
            }
            'b' => {
                out.push('\u{8}');
                continue;
            }
            'f' => {
                out.push('\u{c}');
                continue;
            }
            'n' => {
                out.push('\n');
                continue;
            }
            'r' => {
                out.push('\r');
                continue;
            }
            't' => {
                out.push('\t');
                continue;
            }
            'v' => {
                out.push('\u{b}');
                continue;
            }
            // The octal escape's own character is its first digit.
            '0'..='7' => (3, 8, index - 1),
            'x' => (2, 16, index),
            'u' => (4, 16, index),
            'U' => (8, 16, index),
            _ => {
                out.push(escape);
                continue;
            }
        };
        let digits: String = chars[start..]
            .iter()
            .take(count)
            .take_while(|digit| digit.is_digit(radix))
            .collect();
        if digits.is_empty() {
            out.push('\\');
            out.push(escape);
            continue;
        }
        index = start + digits.len();
        let code = u32::from_str_radix(&digits, radix).expect("only digits") & 0xFFFF;
        out.push(char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER));
    }
    out
}

// `readGitGpgConfig` (`GitGpgConfigUtils.kt:18-26`) with `git config --null --get`, where exit 1 is
// unset. `--bool` gives git's own rules: a key without `=` is true, which 241's reading misses.
pub fn config(repo: &Path) -> Result<SigningConfig, GitError> {
    let read = quiet(repo);
    Ok(SigningConfig {
        sign: value(&read, &["--bool", "commit.gpgSign"])?.as_deref() == Some("true"),
        key: signing_key(&read)?,
        ssh: is_ssh(&read)?,
    })
}

// How a value is read: by git directly, or by a task's git, which can be cancelled.
type Read<'a> = dyn Fn(&[&str]) -> Result<Vec<u8>, GitError> + 'a;

fn quiet(repo: &Path) -> impl Fn(&[&str]) -> Result<Vec<u8>, GitError> + '_ {
    move |args| git::run(repo, args)
}

fn signing_key(read: &Read) -> Result<Option<String>, GitError> {
    Ok(value(read, &["user.signingkey"])?
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty()))
}

fn is_ssh(read: &Read) -> Result<bool, GitError> {
    Ok(value(read, &["gpg.format"])?.as_deref() == Some("ssh"))
}

fn value(read: &Read, name: &[&str]) -> Result<Option<String>, GitError> {
    let (key, types) = name.split_last().expect("a key");
    let mut args = vec!["config", "--null"];
    args.extend_from_slice(types);
    args.extend(["--get", key]);
    match read(&args) {
        Ok(output) => Ok(output
            .iter()
            .position(|byte| *byte == 0)
            .map(|end| String::from_utf8_lossy(&output[..end]).into_owned())),
        Err(GitError::Failed { code: Some(1), .. }) => Ok(None),
        Err(error) => Err(error),
    }
}

// `writeGitGpgConfig` (`GitGpgConfigUtils.kt:82-91`): the local config, which every worktree of
// the repository shares. Unchecking leaves `user.signingkey` as it was. The writes go to the
// Console, as 241's `setSilent(false)`; the reads before them do not.
//
// Decision 4: with `gpg.format=ssh`, only the key already configured (the trimmed value `config`
// shows) can be written: any other is a GPG key ID, which would break every commit.
pub fn write(ctx: &TaskContext, key: Option<&str>) -> Result<(), GitError> {
    let Some(key) = key else {
        return set(ctx, "commit.gpgSign", "false");
    };
    branches::check_argument(key, "The GPG key")?;
    let read = |args: &[&str]| ctx.git(args).map(|output| output.stdout.into_bytes());
    if is_ssh(&read)? && signing_key(&read)?.as_deref() != Some(key) {
        return Err(GitError::Io(SSH_FORMAT.to_string()));
    }
    set(ctx, "commit.gpgSign", "true")?;
    set(ctx, "user.signingkey", key)
}

// `GitConfigUtil.setValue`: exit 1 is not an error (`throwOnError(1)`).
fn set(ctx: &TaskContext, name: &str, value: &str) -> Result<(), GitError> {
    match ctx.git_loud(&["config", name, value]) {
        Ok(_) | Err(GitError::Failed { code: Some(1), .. }) => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{in_task, RepoHarness, TempRepo};
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    // A gpg that records its arguments and whether it had a stdin, prints `stdout` and exits with
    // `code`. Never the real one.
    fn fake_gpg(dir: &Path, stdout: &str, code: i32) -> PathBuf {
        let script = dir.join("fake-gpg");
        let output = dir.join("colons");
        fs::write(&output, stdout).unwrap();
        fs::write(
            &script,
            format!(
                "#!/bin/sh\necho \"$*\" > '{args}'\nif [ -t 0 ] || read -t 1 -r _; then echo input > '{input}'; fi\ncat '{output}'\necho noise >&2\nexit {code}\n",
                args = dir.join("args").display(),
                input = dir.join("input").display(),
                output = output.display(),
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    const COLONS: &str = "\
sec:u:255:22:AAAA111122223333:1700000000:::u:::scESC:::+:::23::0:
fpr:::::::::AAAAAAAAAAAAAAAAAAAAAAAAAAAA111122223333:
grp:::::::::0000000000000000000000000000000000000000:
uid:u::::1700000000::HASH1::Ada Lovelace \\x3cada@example.com\\x3e \\x3a work::::::::::0:
uid:u::::1700000000::HASH2::Second Name <second@example.com>::::::::::0:
ssb:u:255:18:BBBB111122223333:1700000000::::::e:::+:::23:
sec:u:255:22:CCCC111122223333:1700000000:::u:::scESCD:::+:::23::0:
uid:u::::1700000000::HASH3::Disabled <disabled@example.com>::::::::::0:
sec:u:255:22:DDDD111122223333:1700000000:::u:::eE:::+:::23::0:
uid:u::::1700000000::HASH4::Encrypt Only <encrypt@example.com>::::::::::0:
sec:u:255:22:EEEE111122223333:1700000000:::u:::S:::+:::23::0:
";

    #[test]
    fn the_key_list_keeps_signing_keys_and_their_first_uid() {
        let tools = TempDir::new().unwrap();
        let gpg = fake_gpg(tools.path(), COLONS, 0);

        let keys = secret_keys(&gpg).unwrap();

        assert_eq!(
            keys,
            [
                GpgKey {
                    id: "AAAA111122223333".to_string(),
                    user_id: Some("Ada Lovelace <ada@example.com> : work".to_string()),
                },
                // `S` alone (the primary key's whole-key capability) also signs; it has no uid.
                GpgKey {
                    id: "EEEE111122223333".to_string(),
                    user_id: None,
                },
            ]
        );
        assert_eq!(
            fs::read_to_string(tools.path().join("args")).unwrap(),
            "--list-secret-keys --with-colons --fixed-list-mode --batch --no-tty\n"
        );
        assert!(
            !tools.path().join("input").exists(),
            "gpg must have no stdin"
        );
    }

    #[test]
    fn user_ids_are_unescaped_as_241_does() {
        assert_eq!(unescape(r"a\x3ab"), "a:b");
        assert_eq!(unescape(r"tab\there\nline"), "tab\there\nline");
        assert_eq!(unescape(r"\101é\\"), "A\u{e9}\\");
        // What is not an escape stays as it is.
        assert_eq!(unescape(r"\xZZ \q end\"), r"\xZZ q end\");
    }

    #[test]
    fn a_failing_gpg_gives_an_empty_list_and_a_missing_one_an_error() {
        let tools = TempDir::new().unwrap();
        let failing = fake_gpg(tools.path(), "", 2);
        assert_eq!(secret_keys(&failing).unwrap(), []);

        // Like 241, only stdout counts: a gpg that lists keys and then fails still gives them.
        let listing = TempDir::new().unwrap();
        let partial = fake_gpg(listing.path(), COLONS, 2);
        assert_eq!(secret_keys(&partial).unwrap().len(), 2);

        let missing = tools.path().join("no-such-gpg");
        let Err(GitError::Io(message)) = secret_keys(&missing) else {
            panic!("a gpg that cannot start is an error");
        };
        assert_eq!(
            message,
            format!(
                "Cannot run program \"{}\": No such file or directory (os error 2)",
                missing.display()
            )
        );
        let GitError::Io(message) = not_found() else {
            panic!("no gpg found is an error");
        };
        assert_eq!(
            message,
            "Cannot run program \"gpg\": No such file or directory (os error 2)"
        );
    }

    fn executable(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn gpg_program_from_the_config_wins_and_the_fallbacks_are_tried_in_order() {
        let repo = TempRepo::new();
        let tools = TempDir::new().unwrap();
        let [empty, on_path, first, second] = ["empty", "on-path", "first", "second"].map(|name| {
            let dir = tools.path().join(name);
            fs::create_dir(&dir).unwrap();
            dir
        });
        let path: OsString = std::env::join_paths([&empty, &on_path]).unwrap();
        let folders = [first.to_str().unwrap(), second.to_str().unwrap()];

        // An absolute `gpg.program` is taken as it is, even if it is not there: running it says so.
        let configured = tools.path().join("configured-gpg");
        repo.git(&["config", "gpg.program", configured.to_str().unwrap()]);
        assert_eq!(program(repo.path()).unwrap(), Some(configured.clone()));
        let configured = configured.to_str();
        assert_eq!(
            resolve(repo.path(), configured, Some(&path), &folders),
            configured.map(PathBuf::from)
        );

        // A bare name is looked for as `gpg` is; a relative path is the repository's.
        let named = executable(&second, "gpg2");
        assert_eq!(
            resolve(repo.path(), Some("gpg2"), Some(&path), &folders),
            Some(named)
        );
        assert_eq!(
            resolve(repo.path(), Some("tools/gpg"), Some(&path), &folders),
            Some(repo.path().join("tools/gpg"))
        );

        // Without it: `PATH`, then the folders in their order.
        let none = resolve(repo.path(), None, Some(&path), &folders);
        assert_eq!(none, None);
        let last = executable(&second, "gpg");
        assert_eq!(
            resolve(repo.path(), None, Some(&path), &folders),
            Some(last)
        );
        let middle = executable(&first, "gpg");
        assert_eq!(
            resolve(repo.path(), None, Some(&path), &folders),
            Some(middle)
        );
        let found = executable(&on_path, "gpg");
        assert_eq!(
            resolve(repo.path(), None, Some(&path), &folders),
            Some(found.clone())
        );
        assert_eq!(
            resolve(repo.path(), Some(""), Some(&path), &folders),
            Some(found)
        );
        // A folder is not a program, and a relative `PATH` entry is never used.
        fs::create_dir(empty.join("gpg")).unwrap();
        let relative: OsString = std::env::join_paths([Path::new("relative"), &empty]).unwrap();
        assert_eq!(resolve(repo.path(), None, Some(&relative), &[]), None);
    }

    #[test]
    fn gpg_program_expands_a_home_path_and_a_broken_config_is_an_error() {
        let repo = TempRepo::new();
        repo.git(&["config", "gpg.program", "~/bin/gpg"]);
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        assert_eq!(program(repo.path()).unwrap(), Some(home.join("bin/gpg")));

        // git cannot read it: rama does not guess another gpg.
        let file = repo.path().join(".git/config");
        let mut text = fs::read_to_string(&file).unwrap();
        text.push_str("[broken\n");
        fs::write(&file, text).unwrap();
        assert!(
            matches!(
                program(repo.path()),
                Err(GitError::Failed {
                    code: Some(128),
                    ..
                })
            ),
            "{:?}",
            program(repo.path())
        );
    }

    fn local(repo: &TempRepo, name: &str) -> String {
        repo.git(&["config", "--local", "--get", name])
    }

    // `write` runs in a real task of `harness`, in `repo` (the harness's or one of its worktrees).
    fn write_in(harness: &RepoHarness, repo: &Path, key: Option<&str>) -> Result<(), GitError> {
        let key = key.map(str::to_string);
        in_task(&harness.runner, repo, move |ctx| write(ctx, key.as_deref()))
    }

    #[test]
    fn writing_a_key_sets_gpgsign_and_signingkey_locally_and_unchecking_writes_only_false() {
        let harness = RepoHarness::new();
        let repo = &harness.repo;
        repo.commit("a.txt", b"1\n", "one");
        let linked = repo.worktree("linked");

        write_in(&harness, linked.path(), Some("AAAA111122223333")).unwrap();

        // From a linked worktree, as from the main one: the main repository's config.
        let file = fs::read_to_string(repo.path().join(".git/config")).unwrap();
        assert!(file.contains("gpgSign = true"), "{file}");
        assert!(file.contains("signingkey = AAAA111122223333"), "{file}");
        assert_eq!(
            config(linked.path()).unwrap(),
            SigningConfig {
                sign: true,
                key: Some("AAAA111122223333".to_string()),
                ssh: false,
            }
        );
        // The writes show in the Console, as in 241; the reads before them do not.
        assert!(harness.ran("config commit.gpgSign true"));
        assert!(harness.ran("config user.signingkey AAAA111122223333"));
        assert!(!harness.ran("--get gpg.format"));

        write_in(&harness, repo.path(), None).unwrap();

        assert!(harness.ran("config commit.gpgSign false"));
        assert_eq!(local(repo, "commit.gpgSign"), "false\n");
        assert_eq!(local(repo, "user.signingkey"), "AAAA111122223333\n");
        assert_eq!(
            config(repo.path()).unwrap(),
            SigningConfig {
                sign: false,
                key: Some("AAAA111122223333".to_string()),
                ssh: false,
            }
        );

        // Nothing from the interface reaches git as an option.
        let Err(GitError::Io(message)) = write_in(&harness, repo.path(), Some("-x")) else {
            panic!("a key that looks like an option is refused");
        };
        assert_eq!(message, "The GPG key '-x' is not valid");
        assert_eq!(local(repo, "user.signingkey"), "AAAA111122223333\n");
    }

    #[test]
    fn the_config_reads_git_booleans_and_unset_values() {
        let repo = TempRepo::new();
        repo.git(&["config", "--unset", "commit.gpgsign"]);
        assert_eq!(config(repo.path()).unwrap(), SigningConfig::default());

        // A key without `=` is true for git, and so here.
        let file = repo.path().join(".git/config");
        let mut text = fs::read_to_string(&file).unwrap();
        text.push_str("[commit]\n\tgpgSign\n[user]\n\tsigningkey = \"  KEY  \"\n");
        fs::write(&file, text).unwrap();
        assert_eq!(
            config(repo.path()).unwrap(),
            SigningConfig {
                sign: true,
                key: Some("KEY".to_string()),
                ssh: false,
            }
        );

        for (value, sign) in [
            ("yes", true),
            ("On", true),
            ("1", true),
            ("off", false),
            ("", false),
        ] {
            repo.git(&["config", "commit.gpgSign", value]);
            assert_eq!(config(repo.path()).unwrap().sign, sign, "{value:?}");
        }

        repo.git(&["config", "commit.gpgSign", "maybe"]);
        assert!(matches!(config(repo.path()), Err(GitError::Failed { .. })));
    }

    #[test]
    fn with_gpg_format_ssh_writing_a_gpg_key_is_refused_and_nothing_changes() {
        let harness = RepoHarness::new();
        let repo = &harness.repo;
        repo.commit("a.txt", b"1\n", "one");
        repo.sign_commits();
        let before = fs::read_to_string(repo.path().join(".git/config")).unwrap();
        let ssh = config(repo.path()).unwrap();
        assert!(ssh.sign && ssh.ssh);

        let Err(GitError::Io(message)) = write_in(&harness, repo.path(), Some("AAAA111122223333"))
        else {
            panic!("a GPG key is refused while gpg.format is ssh");
        };

        assert_eq!(
            message,
            "Commits are signed with SSH (gpg.format=ssh). Change gpg.format to use a GPG key."
        );
        assert_eq!(
            fs::read_to_string(repo.path().join(".git/config")).unwrap(),
            before
        );
        assert!(!harness.ran("config commit.gpgSign true"));

        // Unchecking still turns signing off.
        write_in(&harness, repo.path(), None).unwrap();
        assert_eq!(local(repo, "commit.gpgsign"), "false\n");
        assert!(config(repo.path()).unwrap().ssh);
    }

    // Decision 4 refuses a GPG key ID, not the SSH key that is already configured: the dialog
    // shows it as it is and gives it back.
    #[test]
    fn with_gpg_format_ssh_the_configured_key_can_be_kept_and_turned_back_on() {
        let harness = RepoHarness::new();
        let repo = &harness.repo;
        repo.commit("a.txt", b"1\n", "one");
        repo.sign_commits();
        let key = config(repo.path()).unwrap().key.unwrap();
        let key = key.as_str();
        let signing = SigningConfig {
            sign: true,
            key: Some(key.to_string()),
            ssh: true,
        };

        // OK with nothing changed writes the same values again.
        write_in(&harness, repo.path(), Some(key)).unwrap();
        assert_eq!(config(repo.path()).unwrap(), signing);
        assert_eq!(local(repo, "user.signingkey"), format!("{key}\n"));

        // Unchecked, then checked again with the same SSH key.
        write_in(&harness, repo.path(), None).unwrap();
        assert!(!config(repo.path()).unwrap().sign);
        write_in(&harness, repo.path(), Some(key)).unwrap();
        assert_eq!(config(repo.path()).unwrap(), signing);
        repo.commit("a.txt", b"2\n", "signed again");
        assert!(repo
            .git(&["cat-file", "commit", "HEAD"])
            .contains("gpgsig "));

        // A GPG key ID is still refused, and nothing changes.
        let before = fs::read_to_string(repo.path().join(".git/config")).unwrap();
        let Err(GitError::Io(message)) = write_in(&harness, repo.path(), Some("AAAA111122223333"))
        else {
            panic!("a GPG key is refused while gpg.format is ssh");
        };
        assert_eq!(
            message,
            "Commits are signed with SSH (gpg.format=ssh). Change gpg.format to use a GPG key."
        );
        assert_eq!(
            fs::read_to_string(repo.path().join(".git/config")).unwrap(),
            before
        );
    }
}
