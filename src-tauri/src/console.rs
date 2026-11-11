use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

pub const CAPACITY: usize = 2_000;
pub const MAX_CHARS: usize = 10_000;

// The `write(...)` commands of GitCommand.java: the ones IntelliJ prints without being asked
// (GitHandler.java:139). The rest, fetch and clone among them, are reads and are only printed
// when whoever launches them asks for it (`TaskContext::git_loud`).
const WRITING: [&str; 19] = [
    "add",
    "checkout",
    "cherry-pick",
    "commit",
    "hash-object",
    "init",
    "merge",
    "mv",
    "pull",
    "push",
    "rebase",
    "reset",
    "restore",
    "revert",
    "rm",
    "stash",
    "submodule",
    "update-index",
    "update-ref",
];
// Read forms of a write command (`GitStashUtils` uses `STASH.readLockingCommand()`). `submodule
// status` is the read Ramify runs after every Update (decision 7), which 241 does not run there.
const READING_FORMS: [(&str, &[&str]); 2] =
    [("stash", &["list", "show"]), ("submodule", &["status"])];
// For these the normal output is shown, not just the errors (`setStdoutSuppressed(false)`).
const SHOWING_STDOUT: [&str; 2] = ["rebase", "commit"];

pub type Listener<T> = Arc<dyn Fn(&T) + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConsoleLine {
    // The project window whose command wrote it: each window's console shows only its own, like
    // IntelliJ's console per project.
    pub owner: String,
    pub time: String,
    pub command: bool,
    pub text: String,
}

// `CAPACITY` lines per project window.
#[derive(Default)]
pub struct Console {
    lines: Mutex<HashMap<String, VecDeque<ConsoleLine>>>,
    listener: Mutex<Option<Listener<ConsoleLine>>>,
}

impl Console {
    pub fn listen(&self, listener: impl Fn(&ConsoleLine) + Send + Sync + 'static) {
        *self.listener.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(listener));
    }

    // What is launched, in the format of `GitImplBase`: `[<folder>] git <arguments>`.
    pub fn command(&self, owner: &str, repo: &Path, args: &[&str]) {
        let folder = repo
            .file_name()
            .unwrap_or(repo.as_os_str())
            .to_string_lossy();
        self.push(owner, true, format!("[{folder}] git {}", args.join(" ")));
    }

    pub fn output(&self, owner: &str, text: &str) {
        self.push(owner, false, text.to_string());
    }

    // The lines of the project window `owner`, oldest first.
    pub fn lines_of(&self, owner: &str) -> Vec<ConsoleLine> {
        let lines = self.lines.lock().unwrap_or_else(PoisonError::into_inner);
        lines
            .get(owner)
            .map(|lines| lines.iter().cloned().collect())
            .unwrap_or_default()
    }

    // The project window `owner` was closed: its lines go with it.
    pub fn forget(&self, owner: &str) {
        self.lines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(owner);
    }

    fn push(&self, owner: &str, command: bool, text: String) {
        let line = ConsoleLine {
            owner: owner.to_string(),
            time: clock(),
            command,
            text: shorten(hide_credentials(&text)),
        };
        {
            let mut all = self.lines.lock().unwrap_or_else(PoisonError::into_inner);
            let lines = all.entry(owner.to_string()).or_default();
            if lines.len() == CAPACITY {
                lines.pop_front();
            }
            lines.push_back(line.clone());
        }
        let listener = self
            .listener
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(listener) = listener {
            listener(&line);
        }
    }
}

// The command name, without the global options (`-c key=value`, `-C folder`, `--...`), and
// its first argument.
pub(crate) fn subcommand<'a>(args: &[&'a str]) -> Option<(&'a str, Option<&'a str>)> {
    let mut args = args.iter().copied();
    while let Some(arg) = args.next() {
        match arg {
            "-c" | "-C" => {
                args.next();
            }
            _ if arg.starts_with('-') => {}
            _ => return Some((arg, args.next())),
        }
    }
    None
}

pub fn writes(args: &[&str]) -> bool {
    subcommand(args).is_some_and(|(name, first)| {
        WRITING.contains(&name)
            && !READING_FORMS
                .iter()
                .any(|(form, firsts)| *form == name && first.is_some_and(|f| firsts.contains(&f)))
    })
}

pub fn shows_stdout(args: &[&str]) -> bool {
    writes(args) && subcommand(args).is_some_and(|(name, _)| SHOWING_STDOUT.contains(&name))
}

// A token goes as the user without `:` (`https://ghp_xxx@github.com/...`). With fewer, it is a name.
const TOKEN_MIN_CHARS: usize = 20;

fn looks_like_token(user: &str) -> bool {
    user.chars().count() >= TOKEN_MIN_CHARS && !user.contains('@')
}

// `https://user:password@host/...` must not reach the console.
pub fn hide_credentials(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(found) = rest.find("://") {
        let (head, tail) = rest.split_at(found + 3);
        result.push_str(head);
        let end = tail
            .find(|c: char| c == '/' || c.is_whitespace())
            .unwrap_or(tail.len());
        let (authority, after) = tail.split_at(end);
        match authority.rfind('@') {
            Some(at) => match authority[..at].split_once(':') {
                Some((user, _)) => {
                    result.push_str(user);
                    result.push_str(":***");
                    result.push_str(&authority[at..]);
                }
                None if looks_like_token(&authority[..at]) => {
                    result.push_str("***");
                    result.push_str(&authority[at..]);
                }
                None => result.push_str(authority),
            },
            None => result.push_str(authority),
        }
        rest = after;
    }
    result.push_str(rest);
    result
}

fn shorten(text: String) -> String {
    if text.chars().count() <= MAX_CHARS {
        return text;
    }
    let mut shortened: String = text.chars().take(MAX_CHARS - 1).collect();
    shortened.push('…');
    shortened
}

// Local time `HH:mm:ss.SSS`, like IntelliJ's console.
fn clock() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = now.as_secs() as libc::time_t;
    // SAFETY: `localtime_r` only writes to `local`, which is a whole `tm` of its own.
    let local = unsafe {
        let mut local: libc::tm = std::mem::zeroed();
        libc::localtime_r(&seconds, &mut local);
        local
    };
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        local.tm_hour,
        local.tm_min,
        local.tm_sec,
        now.subsec_millis()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_clock(time: &str) -> bool {
        let bytes = time.as_bytes();
        bytes.len() == 12
            && [2, 5].iter().all(|&i| bytes[i] == b':')
            && bytes[8] == b'.'
            && bytes
                .iter()
                .enumerate()
                .all(|(i, b)| [2, 5, 8].contains(&i) || b.is_ascii_digit())
    }

    #[test]
    fn console_hides_credentials_in_urls() {
        assert_eq!(
            hide_credentials("https://u:pass@host/r"),
            "https://u:***@host/r"
        );
        assert_eq!(
            hide_credentials("fetch https://a:b@h/x y http://c:d@i:8080/z"),
            "fetch https://a:***@h/x y http://c:***@i:8080/z"
        );
        assert_eq!(
            hide_credentials("https://only@host/r"),
            "https://only@host/r"
        );
        assert_eq!(
            hide_credentials("https://host/r?a=b@c"),
            "https://host/r?a=b@c"
        );
        assert_eq!(hide_credentials("git@host:u/r.git"), "git@host:u/r.git");
    }

    #[test]
    fn console_hides_a_token_as_user() {
        assert_eq!(
            hide_credentials(
                "clone https://ghp_0123456789abcdefghijABCDEFGHIJ012345@github.com/u/r.git done"
            ),
            "clone https://***@github.com/u/r.git done"
        );
        assert_eq!(
            hide_credentials("https://glpat-abcdefghij0123456789@host:8443/r"),
            "https://***@host:8443/r"
        );
    }

    #[test]
    fn console_shows_a_user_that_does_not_look_like_a_token() {
        assert_eq!(
            hide_credentials("https://short.user-1@host/r"),
            "https://short.user-1@host/r"
        );
        assert_eq!(
            hide_credentials("https://jane.doe.smith@company.example@host/r"),
            "https://jane.doe.smith@company.example@host/r"
        );
    }

    #[test]
    fn console_truncates_to_10000_characters() {
        let console = Console::default();
        console.output("main", &"é".repeat(25_000));
        let lines = console.lines_of("main");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text.chars().count(), MAX_CHARS);
        assert!(lines[0].text.ends_with('…'));
    }

    #[test]
    fn console_keeps_the_last_2000_lines() {
        let console = Console::default();
        for index in 0..CAPACITY + 5 {
            console.output("main", &format!("line {index}"));
        }
        let lines = console.lines_of("main");
        assert_eq!(lines.len(), CAPACITY);
        assert_eq!(lines[0].text, "line 5");
    }

    #[test]
    fn the_command_carries_time_folder_and_arguments() {
        let console = Console::default();
        console.command(
            "main",
            Path::new("/tmp/project"),
            &["-c", "a=b", "fetch", "origin"],
        );
        console.output("main", "From /tmp/other");
        let lines = console.lines_of("main");
        assert!(is_clock(&lines[0].time), "{}", lines[0].time);
        assert!(lines[0].command);
        assert_eq!(lines[0].text, "[project] git -c a=b fetch origin");
        assert!(!lines[1].command);
        assert_eq!(lines[1].text, "From /tmp/other");
    }

    #[test]
    fn the_command_hides_credentials_and_lines_are_notified() {
        let console = Console::default();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        console.listen(move |line| sink.lock().unwrap().push(line.clone()));
        console.command("main", Path::new("/r"), &["push", "https://u:pass@host/r"]);
        console.output("main", "fatal: https://u:pass@host/r does not respond");
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].text, "[r] git push https://u:***@host/r");
        assert_eq!(seen[1].text, "fatal: https://u:***@host/r does not respond");
        assert_eq!(*seen, console.lines_of("main"));
    }

    #[test]
    fn lines_are_kept_per_window() {
        let console = Console::default();
        console.command("main", Path::new("/repos/a"), &["fetch"]);
        console.output("main-2", "from b");
        let texts = |owner: &str| -> Vec<(String, String)> {
            console
                .lines_of(owner)
                .into_iter()
                .map(|line| (line.owner, line.text))
                .collect()
        };

        assert_eq!(
            texts("main"),
            [("main".to_string(), "[a] git fetch".to_string())]
        );
        assert_eq!(
            texts("main-2"),
            [("main-2".to_string(), "from b".to_string())]
        );
        assert!(console.lines_of("main-3").is_empty());

        // The limit is per window: a busy window does not push out another's lines.
        for index in 0..CAPACITY + 5 {
            console.output("main", &format!("line {index}"));
        }
        let main = console.lines_of("main");
        assert_eq!(main.len(), CAPACITY);
        assert_eq!(main[0].text, "line 5");
        assert_eq!(texts("main-2").len(), 1);
    }

    #[test]
    fn only_the_commands_that_write_are_shown() {
        // The `write(...)` ones of GitCommand.java.
        for args in [
            &["pull", "--no-stat"][..],
            &["-c", "a=b", "commit", "-m", "x"],
            &["--literal-pathspecs", "-C", "/x", "push", "origin", "main"],
            &["update-index", "--refresh"],
            &["hash-object", "-w", "a"],
            &["submodule", "update"],
            &["stash"],
            &["stash", "push", "-m", "x"],
            &["stash", "pop"],
        ] {
            assert!(writes(args), "{args:?}");
        }
        // Reads, also the ones IntelliJ treats as such even if they write (clone, branch...).
        for args in [
            &["fetch", "origin"][..],
            &["clone", "url", "dir"],
            &["branch", "new"],
            &["tag", "v1"],
            &["remote", "add", "o", "u"],
            &["stash", "list"],
            &["-c", "a=b", "stash", "show", "-p"],
            &["submodule", "status", "--recursive"],
            &["rev-list", "--count", "HEAD"],
            &["-c", "a=b", "diff", "--stat"],
            &["ls-files"],
            &["config", "--get", "user.name"],
            &["merge-base", "a", "b"],
            &[],
        ] {
            assert!(!writes(args), "{args:?}");
        }
    }

    #[test]
    fn normal_output_is_only_shown_in_rebase_and_commit() {
        assert!(shows_stdout(&["-c", "a=b", "rebase", "main"]));
        assert!(shows_stdout(&["commit"]));
        assert!(!shows_stdout(&["fetch"]));
        assert!(!shows_stdout(&["push"]));
        assert!(!shows_stdout(&["stash", "list"]));
    }
}
