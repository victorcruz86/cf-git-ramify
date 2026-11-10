// Long tasks (fetch, pull, push...) off the interface thread: they are serialized according to what
// they touch, can be cancelled, and report their progress and output while they run.
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, TryLockError};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::askpass::{self, ClientEnv};
use crate::commands::{ApiError, RepoLocks};
use crate::console::{self, Console, Listener};
use crate::editor::{self, Ask, EditorHandler};
use crate::git::{self, GitError};

pub type TaskId = u64;

// Like `GitTextHandler`: it is asked to finish and, if it does not within a minute, it is killed.
const KILL_GRACE: Duration = Duration::from_secs(60);
// How often to check whether it was cancelled (IntelliJ, 50 ms) and whether the lock was released.
const POLL: Duration = Duration::from_millis(20);
const CREDENTIAL_MANAGER_PROMPTS: &str = "GCM_INTERACTIVE";
const READ_BUFFER_BYTES: usize = 64 * 1024;
// The options IntelliJ adds to every command: without them, paths with accents come out quoted
// and a broken signature dirties the output.
const OPTIONS: [&str; 4] = [
    "-c",
    "core.quotepath=false",
    "-c",
    "log.showSignature=false",
];
// Emptying the helper list: git no longer consults them and asks for everything through the dialog.
const NO_CREDENTIAL_HELPER: [&str; 2] = ["-c", "credential.helper="];
// After the editor request is withdrawn, git exits on its own (the editor exits with 1 and git undoes what
// it started). If it does not do so within this period, it is treated like any other.
const EDITOR_EXIT_GRACE: Duration = Duration::from_secs(5);

// How git's editor is prepared in a command.
#[derive(Clone, Copy)]
enum Editing<'a> {
    // The usual one, the user's.
    Untouched,
    // Nobody to answer: the editor refuses and git fails instead of waiting.
    Refused,
    Registered(&'a editor::Registration),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TaskKind {
    // Waits for no one.
    Read,
    // Changes the index or the working tree: one at a time per repository, and together with commit,
    // rollback and the rest, which take the same lock.
    Write,
    // Talks to a remote: one at a time per remote, without touching the index, so a commit does not wait.
    Remote(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    Running,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TaskEvent {
    pub id: TaskId,
    pub title: String,
    pub text: String,
    pub fraction: Option<f32>,
    pub state: TaskState,
    pub kind: Option<&'static str>,
    pub value: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitOutput {
    pub stdout: String,
    pub stderr: String,
}

struct Task {
    id: TaskId,
    title: String,
    cancelled: AtomicBool,
    // Waits for the repository lock or holds it: quitting must wait for it.
    writing: AtomicBool,
    // Past this instant it counts as cancelled.
    deadline: Option<Instant>,
    progress: Mutex<(String, Option<f32>)>,
}

// One lock per repository and remote.
type RemoteLocks = HashMap<(PathBuf, String), Arc<Mutex<()>>>;

impl Task {
    fn event(&self, state: TaskState, text: String, fraction: Option<f32>) -> TaskEvent {
        TaskEvent {
            id: self.id,
            title: self.title.clone(),
            text,
            fraction,
            state,
            kind: None,
            value: None,
        }
    }
}

struct Shared {
    locks: Arc<RepoLocks>,
    console: Arc<Console>,
    // `None` is the git from the settings. Only tests change it.
    binary: Option<PathBuf>,
    grace: Duration,
    askpass: OnceLock<ClientEnv>,
    editor: OnceLock<editor::Handle>,
    credential_helper: AtomicBool,
    staging_area: AtomicBool,
    cherry_pick_suffix: AtomicBool,
    closing: AtomicBool,
    listener: Mutex<Option<Listener<TaskEvent>>>,
    running: Mutex<HashMap<TaskId, Arc<Task>>>,
    remotes: Mutex<RemoteLocks>,
    next: AtomicU64,
}

impl Shared {
    fn emit(&self, event: TaskEvent) {
        let listener = self
            .listener
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(listener) = listener {
            listener(&event);
        }
    }

    fn remote_lock(&self, repo: &Path, remote: String) -> Arc<Mutex<()>> {
        let mut remotes = self.remotes.lock().unwrap_or_else(PoisonError::into_inner);
        remotes
            .entry((repo.to_path_buf(), remote))
            .or_default()
            .clone()
    }
}

pub struct TaskContext {
    shared: Arc<Shared>,
    task: Arc<Task>,
    repo: PathBuf,
    // No one is waiting for it: it never asks the user anything.
    quiet: bool,
}

impl TaskContext {
    pub fn repo(&self) -> &Path {
        &self.repo
    }

    pub fn staging_area(&self) -> bool {
        self.shared.staging_area.load(Ordering::SeqCst)
    }

    // `shouldAddSuffixToCherryPicksOfPublishedCommits`.
    pub fn cherry_pick_suffix(&self) -> bool {
        self.shared.cherry_pick_suffix.load(Ordering::SeqCst)
    }

    // The progress text of the task (`ProgressIndicator.setText`), until git reports its own.
    pub fn set_text(&self, text: &str) {
        self.report(text.to_string(), None);
    }

    // What the task shows now: a step that shows a text of its own gives this back when it ends.
    pub fn progress(&self) -> (String, Option<f32>) {
        self.task
            .progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn restore_progress(&self, (text, fraction): (String, Option<f32>)) {
        self.report(text, fraction);
    }

    // A line of rama's own in the Console, among git's output.
    pub fn console_line(&self, text: &str) {
        self.shared.console.output(text);
    }

    pub fn check_cancelled(&self) -> Result<(), GitError> {
        let expired = self
            .task
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline);
        if expired || self.task.cancelled.load(Ordering::SeqCst) {
            Err(GitError::Cancelled)
        } else {
            Ok(())
        }
    }

    // A git command of this task. Each child goes in its own process group: cancelling also kills
    // whatever git launches (ssh, hooks, credentials).
    pub fn git(&self, args: &[&str]) -> Result<GitOutput, GitError> {
        self.run_git(args, false, None, true, Editing::Untouched, &[])
    }

    // For `--stdin`: the list of a large selection does not fit in the arguments.
    pub fn git_with_input(&self, args: &[&str], input: &[u8]) -> Result<GitOutput, GitError> {
        self.run_git(args, false, Some(input), true, Editing::Untouched, &[])
    }

    // Like IntelliJ's `setSilent(false)` + `setStdoutSuppressed(false)`, for the reads that are
    // shown anyway (fetch, clone): the command and all its output go to the console.
    pub fn git_loud(&self, args: &[&str]) -> Result<GitOutput, GitError> {
        self.run_git(args, true, None, true, Editing::Untouched, &[])
    }

    // Like `git_loud`, with extra environment variables, set after the task's own: for a git that
    // must run as git itself would run it (`GIT_PROTOCOL_FROM_USER=0` in a submodule's fetch).
    pub fn git_loud_with_env(
        &self,
        args: &[&str],
        env: &[(&str, &str)],
    ) -> Result<GitOutput, GitError> {
        self.run_git(args, true, None, true, Editing::Untouched, env)
    }

    // Like `git_loud`, with an input: `git apply` reads the patch from there.
    pub fn git_loud_with_input(&self, args: &[&str], input: &[u8]) -> Result<GitOutput, GitError> {
        self.run_git(args, true, Some(input), true, Editing::Untouched, &[])
    }

    // What cannot be left half done, like `executeNonCancelableSection`: saving the changes and
    // giving them back. Cancelling here would leave them in the stash with no one expecting them.
    pub fn git_to_the_end(&self, args: &[&str]) -> Result<GitOutput, GitError> {
        self.run_git(args, false, None, false, Editing::Untouched, &[])
    }

    // For commands that may open an editor (`pull.rebase=interactive`, a rebase, a reword):
    // git's editor is this binary and what it asks is answered by `handler`, without a terminal. Like
    // `git_loud`, it shows the command and its output. A quiet task, or one without a server, has no one
    // to ask: the editor refuses and git fails instead of waiting.
    pub fn git_editing(
        &self,
        args: &[&str],
        handler: Arc<dyn EditorHandler>,
    ) -> Result<GitOutput, GitError> {
        let Some(editor) = self.shared.editor.get().filter(|_| !self.quiet) else {
            return self.run_git(args, true, None, true, Editing::Refused, &[]);
        };
        let output = self.git(&["rev-parse", "--absolute-git-dir"])?;
        let git_dir = PathBuf::from(output.stdout.trim_end_matches('\n'));
        let registration = editor.register(handler, git_dir);
        self.run_git(args, true, None, true, Editing::Registered(&registration), &[])
    }

    // The `git_editing` handler that asks the main window.
    pub fn asking_editor(&self) -> Arc<dyn EditorHandler> {
        Arc::new(self.asking())
    }

    // The same, for whoever chooses how the request is shown (`Ask::prompt`).
    pub fn asking(&self) -> Ask {
        Ask::new(self.shared.editor.get().cloned())
    }

    fn run_git(
        &self,
        args: &[&str],
        loud: bool,
        input: Option<&[u8]>,
        cancellable: bool,
        editing: Editing,
        env: &[(&str, &str)],
    ) -> Result<GitOutput, GitError> {
        if cancellable {
            self.check_cancelled()?;
        }
        let binary = self.binary();
        let full = self.full_args(args);
        let mut command = self.command(&binary, &full)?;
        command.envs(env.iter().copied());
        match editing {
            Editing::Untouched => {}
            Editing::Refused => editor::refuse(&mut command),
            Editing::Registered(registration) => registration.apply(&mut command),
        }
        if input.is_some() {
            command.stdin(Stdio::piped());
        }
        let prints = loud || console::writes(args);
        if prints {
            self.shared.console.command(&self.repo, &full);
        }
        let mut child = command
            .spawn()
            .map_err(|error| git::spawn_error(&binary, error))?;
        let pid = child.id();
        let stdout = child.stdout.take().expect("stdout with pipe");
        let stderr = child.stderr.take().expect("stderr with pipe");
        let stdin = child.stdin.take();
        let shows_stdout = loud || console::shows_stdout(args);
        let (status, stdout, stderr) = thread::scope(|scope| {
            if let (Some(mut stdin), Some(input)) = (stdin, input) {
                // If git is cancelled midway, the write error says nothing new.
                scope.spawn(move || {
                    let _ = stdin.write_all(input);
                });
            }
            let stdout = scope.spawn(move || self.read_stdout(stdout, shows_stdout));
            let stderr = scope.spawn(move || self.read_stderr(stderr, prints));
            let (send, exited) = mpsc::channel();
            scope.spawn(move || {
                let _ = send.send(child.wait());
            });
            let status = self.wait(pid, &exited, cancellable, editing);
            (
                status,
                stdout.join().expect("the stdout thread must not fail"),
                stderr.join().expect("the stderr thread must not fail"),
            )
        });
        let status = status.map_err(git::io_error)?;
        let (stdout, stderr) = (
            stdout.map_err(git::io_error)?,
            stderr.map_err(git::io_error)?,
        );
        if cancellable && !status.success() {
            self.check_cancelled()?;
        }
        git::check_status(
            &self.repo,
            args,
            status,
            stdout.as_bytes(),
            stderr.as_bytes(),
        )?;
        Ok(GitOutput { stdout, stderr })
    }

    fn binary(&self) -> PathBuf {
        self.shared.binary.clone().unwrap_or_else(git::git_binary)
    }

    // Like `GitHandlerAuthenticationManager`: `git config` does not go through credentials.
    fn full_args<'a>(&self, args: &[&'a str]) -> Vec<&'a str> {
        let is_config = console::subcommand(args).is_some_and(|(name, _)| name == "config");
        let no_helper = !self.shared.credential_helper.load(Ordering::SeqCst) && !is_config;
        let helper: &[&'a str] = if no_helper {
            &NO_CREDENTIAL_HELPER
        } else {
            &[]
        };
        helper.iter().chain(&OPTIONS).chain(args).copied().collect()
    }

    // The only place where a task's command is assembled: the git environment of every task comes
    // from here. Unless input is given, the child has no stdin, and goes in its own group: a prompt
    // through `/dev/tty` would stop it forever, so all of them go through askpass.
    fn command(&self, binary: &Path, args: &[&str]) -> Result<Command, GitError> {
        let mut command = git::command(binary, &self.repo, args)?;
        command
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        match self.shared.askpass.get().filter(|_| !self.quiet) {
            Some(askpass) => askpass.apply(&mut command),
            None => askpass::refuse_prompts(&mut command),
        }
        if self.quiet {
            // Git Credential Manager's helper opens its own window if it is not told this.
            command.env(CREDENTIAL_MANAGER_PROMPTS, "never");
        }
        Ok(command)
    }

    // Waits for the child and, if cancelled, asks it to finish (SIGTERM to the group) and, once the
    // deadline passes, kills it (SIGKILL).
    fn wait(
        &self,
        pid: u32,
        exited: &mpsc::Receiver<io::Result<ExitStatus>>,
        cancellable: bool,
        editing: Editing,
    ) -> io::Result<ExitStatus> {
        let mut asked = None;
        let mut killed = false;
        let mut withdrawn = false;
        // Since when we are waiting for git to exit on its own, because it was stopped in the editor.
        let mut waiting_editor: Option<Instant> = None;
        loop {
            match exited.recv_timeout(POLL) {
                Ok(status) => return status,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::other("the git process was lost"))
                }
            }
            if !cancellable || self.check_cancelled().is_ok() {
                continue;
            }
            // A git stopped in the editor, killed with SIGTERM, leaves the rebase half done. If the
            // request is withdrawn, the editor exits with 1 and git undoes it itself, as when the
            // dialog is cancelled. If it was not waiting for the editor, it is cancelled as usual.
            if let (Editing::Registered(registration), false) = (editing, withdrawn) {
                withdrawn = true;
                if registration.withdraw() {
                    waiting_editor = Some(Instant::now());
                }
            }
            if waiting_editor.is_some_and(|at| at.elapsed() < EDITOR_EXIT_GRACE) {
                continue;
            }
            match asked {
                None => {
                    signal_group(pid, libc::SIGTERM);
                    asked = Some(Instant::now());
                }
                Some(at) if !killed && at.elapsed() >= self.shared.grace => {
                    signal_group(pid, libc::SIGKILL);
                    killed = true;
                }
                Some(_) => {}
            }
        }
    }

    fn read_stdout(&self, mut stdout: impl Read, show: bool) -> io::Result<String> {
        let mut bytes = Vec::new();
        let mut lines = Segments::new(false);
        let mut buffer = vec![0; READ_BUFFER_BYTES];
        loop {
            let read = stdout.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
            if show {
                lines.feed(&buffer[..read], |line| self.shared.console.output(line));
            }
        }
        if show {
            lines.finish(|line| self.shared.console.output(line));
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    // Git's progress (`Receiving objects:  45%`) arrives separated by `\r` and is not output: it reports
    // how far along it is and does not go into the console or the error text.
    fn read_stderr(&self, mut stderr: impl Read, show: bool) -> io::Result<String> {
        let mut collected = String::new();
        let mut lines = Segments::new(true);
        let mut buffer = vec![0; READ_BUFFER_BYTES];
        let mut handle = |line: &str| {
            if let Some((text, fraction)) = progress(line) {
                self.report(text, Some(fraction));
                return;
            }
            collected.push_str(line);
            collected.push('\n');
            if show {
                self.shared.console.output(line);
            }
        };
        loop {
            let read = stderr.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            lines.feed(&buffer[..read], &mut handle);
        }
        lines.finish(&mut handle);
        Ok(collected)
    }

    fn report(&self, text: String, fraction: Option<f32>) {
        if self.quiet {
            return;
        }
        {
            let mut progress = self
                .task
                .progress
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if *progress == (text.clone(), fraction) {
                return;
            }
            *progress = (text.clone(), fraction);
        }
        self.shared
            .emit(self.task.event(TaskState::Running, text, fraction));
    }
}

fn signal_group(pid: u32, signal: i32) {
    // SAFETY: `kill` only sends a signal. With the negative sign it goes to the group, which the child created with
    // `process_group(0)`; if it no longer exists, it simply fails.
    unsafe {
        libc::kill(-(pid as i32), signal);
    }
}

// Splits the output into lines without waiting for git to close it. In stderr, `\r` also ends a
// line: that is how progress is seen.
struct Segments {
    pending: Vec<u8>,
    carriage: bool,
}

impl Segments {
    fn new(carriage: bool) -> Self {
        Self {
            pending: Vec::new(),
            carriage,
        }
    }

    // Blank lines are not passed on.
    fn feed(&mut self, chunk: &[u8], mut on_line: impl FnMut(&str)) {
        for &byte in chunk {
            if byte == b'\n' || (self.carriage && byte == b'\r') {
                self.flush(&mut on_line);
            } else {
                self.pending.push(byte);
            }
        }
    }

    fn finish(&mut self, mut on_line: impl FnMut(&str)) {
        self.flush(&mut on_line);
    }

    fn flush(&mut self, on_line: &mut impl FnMut(&str)) {
        let line = String::from_utf8_lossy(&self.pending);
        if !line.trim().is_empty() {
            on_line(&line);
        }
        self.pending.clear();
    }
}

// `Label:   45% (3/7)`, with or without the `remote: ` in front.
fn progress(line: &str) -> Option<(String, f32)> {
    let line = line.trim();
    let line = line.strip_prefix("remote:").map_or(line, str::trim_start);
    let percent = line.find('%')?;
    let start = line[..percent]
        .char_indices()
        .rev()
        .find(|(_, c)| !c.is_ascii_digit())
        .map_or(0, |(index, c)| index + c.len_utf8());
    let value: u32 = line[start..percent].parse().ok()?;
    if value > 100 || !line[..start].trim_end().ends_with(':') {
        return None;
    }
    Some((line.to_string(), value as f32 / 100.0))
}

pub struct Runner(Arc<Shared>);

impl Runner {
    pub fn new(locks: Arc<RepoLocks>, console: Arc<Console>) -> Self {
        Self::build(locks, console, None, KILL_GRACE)
    }

    #[cfg(test)]
    pub(crate) fn with(
        locks: Arc<RepoLocks>,
        console: Arc<Console>,
        binary: Option<PathBuf>,
        grace: Duration,
    ) -> Self {
        Self::build(locks, console, binary, grace)
    }

    fn build(
        locks: Arc<RepoLocks>,
        console: Arc<Console>,
        binary: Option<PathBuf>,
        grace: Duration,
    ) -> Self {
        Self(Arc::new(Shared {
            locks,
            console,
            binary,
            grace,
            askpass: OnceLock::new(),
            editor: OnceLock::new(),
            credential_helper: AtomicBool::new(true),
            staging_area: AtomicBool::new(false),
            cherry_pick_suffix: AtomicBool::new(true),
            closing: AtomicBool::new(false),
            listener: Mutex::new(None),
            running: Mutex::new(HashMap::new()),
            remotes: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
        }))
    }

    // Without this, the only thing visible would be git's error when asking for credentials, which does not say the cause.
    pub fn askpass_unavailable(&self, error: &io::Error) {
        self.0.console.output(&format!(
            "Could not prepare the credentials dialog: {error}"
        ));
    }

    pub fn use_askpass(&self, client: ClientEnv) {
        let _ = self.0.askpass.set(client);
    }

    pub fn editor_unavailable(&self, error: &io::Error) {
        self.0
            .console
            .output(&format!("Could not prepare the git editor: {error}"));
    }

    pub fn use_editor(&self, editor: editor::Handle) {
        let _ = self.0.editor.set(editor);
    }

    // It is read on every command: a change in the settings takes effect without a restart.
    pub fn set_credential_helper(&self, enabled: bool) {
        self.0.credential_helper.store(enabled, Ordering::SeqCst);
    }

    // Like the previous one: `isStagingAreaAvailable` decides whether what was stashed comes back with `--index`.
    pub fn set_staging_area(&self, enabled: bool) {
        self.0.staging_area.store(enabled, Ordering::SeqCst);
    }

    // Like the previous ones: `Add the 'cherry-picked from <hash>' suffix…` takes effect without a restart.
    pub fn set_cherry_pick_suffix(&self, enabled: bool) {
        self.0.cherry_pick_suffix.store(enabled, Ordering::SeqCst);
    }

    pub fn listen(&self, listener: impl Fn(&TaskEvent) + Send + Sync + 'static) {
        *self
            .0
            .listener
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(listener));
    }

    // The result of `work` does not leave from here: whoever needs the value delivers it from the task
    // itself. Here we only report how it ended.
    pub fn start<T: Serialize>(
        &self,
        repo: &Path,
        kind: TaskKind,
        title: String,
        work: impl FnOnce(&TaskContext) -> Result<T, GitError> + Send + 'static,
    ) -> TaskId {
        let shared = self.0.clone();
        let task = Arc::new(Task {
            id: shared.next.fetch_add(1, Ordering::SeqCst),
            title,
            cancelled: AtomicBool::new(false),
            writing: AtomicBool::new(false),
            deadline: None,
            progress: Mutex::new((String::new(), None)),
        });
        let id = task.id;
        shared
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, task.clone());
        shared.emit(task.event(TaskState::Running, String::new(), None));
        let context = TaskContext {
            shared,
            task,
            repo: repo.to_path_buf(),
            quiet: false,
        };
        thread::spawn(move || {
            let outcome = context.run(kind, work);
            context.finish(outcome);
        });
        id
    }

    // Background work, on this same thread: it does not show in the status bar, does not wait for its turn, never
    // asks the user anything and is cut off after `limit`, so a hung remote does not hold it.
    pub fn quiet<T>(
        &self,
        repo: &Path,
        limit: Duration,
        work: impl FnOnce(&TaskContext) -> Result<T, GitError>,
    ) -> Result<T, GitError> {
        let shared = self.0.clone();
        let task = Arc::new(Task {
            id: shared.next.fetch_add(1, Ordering::SeqCst),
            title: String::new(),
            cancelled: AtomicBool::new(false),
            writing: AtomicBool::new(false),
            deadline: Some(Instant::now() + limit),
            progress: Mutex::new((String::new(), None)),
        });
        work(&TaskContext {
            shared,
            task,
            repo: repo.to_path_buf(),
            quiet: true,
        })
    }

    // On quit: nobody starts writing, whoever is writing is cancelled and waited for until it finishes, which is
    // what gives back what was stashed. `false` if the deadline passed. Reads, fetch included, do not
    // leave anything half done and are not waited for.
    pub fn finish_writes(&self, limit: Duration) -> bool {
        self.0.closing.store(true, Ordering::SeqCst);
        let deadline = Instant::now() + limit;
        loop {
            let writing = {
                let running = self
                    .0
                    .running
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                let writers = running
                    .values()
                    .filter(|task| task.writing.load(Ordering::SeqCst));
                writers
                    .map(|task| task.cancelled.store(true, Ordering::SeqCst))
                    .count()
            };
            if writing == 0 {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(POLL);
        }
    }

    pub fn cancel(&self, id: TaskId) {
        let running = self
            .0
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(task) = running.get(&id) {
            task.cancelled.store(true, Ordering::SeqCst);
        }
    }
}

impl TaskContext {
    // `None`: it was cancelled while waiting for its turn and never got to start.
    fn run<T>(
        &self,
        kind: TaskKind,
        work: impl FnOnce(&TaskContext) -> Result<T, GitError>,
    ) -> Option<Result<T, GitError>> {
        match kind {
            TaskKind::Read => Some(work(self)),
            TaskKind::Write => self.exclusive(|| work(self)),
            TaskKind::Remote(name) => self.on_remote(&name, || work(self)),
        }
    }

    // What `TaskKind::Write` gives the whole task, for a task that only needs a piece: Update
    // Project must not keep commits stopped while it talks to the remote. `None`: it was cancelled
    // while waiting for its turn.
    pub fn exclusive<T>(&self, work: impl FnOnce() -> T) -> Option<T> {
        // Before looking at `closing`: `finish_writes` sets it before looking at this, and one of the two sees
        // the other.
        self.task.writing.store(true, Ordering::SeqCst);
        let ran = self.take_turn(work);
        self.task.writing.store(false, Ordering::SeqCst);
        ran
    }

    fn take_turn<T>(&self, work: impl FnOnce() -> T) -> Option<T> {
        let mut work = Some(work);
        loop {
            self.check_cancelled().ok()?;
            if self.shared.closing.load(Ordering::SeqCst) {
                return None;
            }
            let ran = self
                .shared
                .locks
                .try_run(&self.repo, POLL, || (work.take().expect("runs once"))());
            if ran.is_some() {
                return ran;
            }
        }
    }

    // The same with `TaskKind::Remote`, for a task that talks to several remotes.
    pub fn on_remote<T>(&self, remote: &str, work: impl FnOnce() -> T) -> Option<T> {
        let lock = self.shared.remote_lock(&self.repo, remote.to_string());
        loop {
            self.check_cancelled().ok()?;
            match lock.try_lock() {
                Ok(_held) => return Some(work()),
                Err(TryLockError::Poisoned(error)) => {
                    let _held = error.into_inner();
                    return Some(work());
                }
                Err(TryLockError::WouldBlock) => thread::sleep(POLL),
            }
        }
    }

    // The value of `work` and the type of its error travel in the final event: the interface tells
    // a rejection from an authentication failure without reading the message.
    fn finish<T: Serialize>(&self, outcome: Option<Result<T, GitError>>) {
        let event = match outcome {
            None | Some(Err(GitError::Cancelled)) => {
                let mut event = self.task.event(TaskState::Cancelled, String::new(), None);
                event.kind = Some("cancelled");
                event
            }
            Some(Ok(value)) => match serde_json::to_value(value) {
                Ok(value) => {
                    let mut event = self.task.event(TaskState::Done, String::new(), None);
                    event.value = Some(value);
                    event
                }
                Err(error) => self.failure(ApiError {
                    kind: "failed",
                    message: error.to_string(),
                    details: None,
                }),
            },
            Some(Err(error)) => self.failure(ApiError::from(error)),
        };
        self.shared
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.task.id);
        self.shared.emit(event);
    }

    fn failure(&self, error: ApiError) -> TaskEvent {
        let mut event = self.task.event(TaskState::Failed, error.message, None);
        event.kind = Some(error.kind);
        event
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempRepo;
    use std::cell::RefCell;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{self, Receiver};
    use std::thread;
    use std::time::Instant;
    use tempfile::TempDir;

    const WAIT: Duration = Duration::from_secs(10);

    struct Harness {
        runner: Runner,
        locks: Arc<RepoLocks>,
        console: Arc<Console>,
        events: Receiver<TaskEvent>,
        seen: RefCell<Vec<TaskEvent>>,
        dir: TempDir,
    }

    impl Harness {
        fn new() -> Self {
            Self::with(None, Duration::from_secs(60))
        }

        fn with(binary: Option<PathBuf>, grace: Duration) -> Self {
            let locks = Arc::new(RepoLocks::default());
            let console = Arc::new(Console::default());
            let runner = Runner::with(locks.clone(), console.clone(), binary, grace);
            let (send, events) = mpsc::channel();
            let send = Mutex::new(send);
            runner.listen(move |event| {
                let _ = send.lock().unwrap().send(event.clone());
            });
            Self {
                runner,
                locks,
                console,
                events,
                seen: RefCell::new(Vec::new()),
                dir: TempDir::new().unwrap(),
            }
        }

        // A fake `git`: the test decides what it does without depending on the real one.
        fn fake(body: &str) -> Self {
            Self::fake_with_grace(body, Duration::from_secs(60))
        }

        fn fake_with_grace(body: &str, grace: Duration) -> Self {
            let scripts = TempDir::new().unwrap();
            let script = scripts.path().join("git");
            fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
            // The scripts directory must live as long as the test.
            let harness = Self::with(Some(script), grace);
            std::mem::forget(scripts);
            harness
        }

        fn repo(&self) -> &Path {
            self.dir.path()
        }

        fn git(
            &self,
            kind: TaskKind,
            args: &[&str],
        ) -> (TaskId, Receiver<Result<GitOutput, GitError>>) {
            let (send, result) = mpsc::channel();
            let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
            let id = self
                .runner
                .start(self.repo(), kind, "test".to_string(), move |ctx| {
                    let args: Vec<&str> = args.iter().map(String::as_str).collect();
                    let output = ctx.git(&args);
                    let _ = send.send(output.clone());
                    output
                });
            (id, result)
        }

        // The events of `id` until it finishes, the last one included. Those of other tasks are
        // kept for when they are asked about.
        fn until_finished(&self, id: TaskId) -> Vec<TaskEvent> {
            loop {
                let mut seen = self.seen.borrow_mut();
                let own: Vec<_> = seen
                    .iter()
                    .filter(|event| event.id == id)
                    .cloned()
                    .collect();
                if own
                    .last()
                    .is_some_and(|event| event.state != TaskState::Running)
                {
                    return own;
                }
                seen.push(
                    self.events
                        .recv_timeout(WAIT)
                        .expect("the task did not finish"),
                );
            }
        }
    }

    fn overlapping(harness: &Harness, first: TaskKind, second: TaskKind) -> (usize, Vec<TaskId>) {
        let current = Arc::new(AtomicUsize::new(0));
        let most = Arc::new(AtomicUsize::new(0));
        let ids: Vec<TaskId> = [first, second]
            .into_iter()
            .map(|kind| {
                let (current, most) = (current.clone(), most.clone());
                harness
                    .runner
                    .start(harness.repo(), kind, "wait".to_string(), move |_| {
                        let now = current.fetch_add(1, Ordering::SeqCst) + 1;
                        most.fetch_max(now, Ordering::SeqCst);
                        thread::sleep(Duration::from_millis(300));
                        current.fetch_sub(1, Ordering::SeqCst);
                        Ok(())
                    })
            })
            .collect();
        for id in &ids {
            harness.until_finished(*id);
        }
        (most.load(Ordering::SeqCst), ids)
    }

    #[test]
    fn write_task_waits_for_the_previous_one() {
        let harness = Harness::new();
        let (most, _) = overlapping(&harness, TaskKind::Write, TaskKind::Write);
        assert_eq!(most, 1);
    }

    #[test]
    fn write_task_waits_for_the_repository_lock() {
        let harness = Harness::new();
        let (held, release) = mpsc::channel::<()>();
        let (started, ready) = mpsc::channel();
        let locks = harness.locks.clone();
        let repo = harness.repo().to_path_buf();
        let holder = thread::spawn(move || {
            locks.run(&repo, || {
                started.send(()).unwrap();
                release.recv().unwrap();
            })
        });
        ready.recv_timeout(WAIT).unwrap();
        let (ran, did_run) = mpsc::channel();
        let id = harness.runner.start(
            harness.repo(),
            TaskKind::Write,
            "writes".to_string(),
            move |_| {
                ran.send(()).unwrap();
                Ok(())
            },
        );

        assert!(did_run.recv_timeout(Duration::from_millis(300)).is_err());
        held.send(()).unwrap();
        holder.join().unwrap();
        harness.until_finished(id);
        assert!(did_run.try_recv().is_ok());
    }

    #[test]
    fn read_task_does_not_wait() {
        let harness = Harness::new();
        let (release, held) = mpsc::channel::<()>();
        let (started, ready) = mpsc::channel();
        let write = harness.runner.start(
            harness.repo(),
            TaskKind::Write,
            "writes".to_string(),
            move |_| {
                started.send(()).unwrap();
                held.recv_timeout(WAIT).unwrap();
                Ok(())
            },
        );
        ready.recv_timeout(WAIT).unwrap();
        let (ran, did_run) = mpsc::channel();

        let read = harness.runner.start(
            harness.repo(),
            TaskKind::Read,
            "reads".to_string(),
            move |_| {
                ran.send(()).unwrap();
                Ok(())
            },
        );

        did_run
            .recv_timeout(Duration::from_secs(2))
            .expect("the read waited for the write");
        harness.until_finished(read);
        release.send(()).unwrap();
        harness.until_finished(write);
    }

    #[test]
    fn remote_task_serializes_per_remote() {
        let harness = Harness::new();
        let (most, _) = overlapping(
            &harness,
            TaskKind::Remote("origin".into()),
            TaskKind::Remote("origin".into()),
        );
        assert_eq!(most, 1);

        let (most, _) = overlapping(
            &harness,
            TaskKind::Remote("origin".into()),
            TaskKind::Remote("other".into()),
        );
        assert_eq!(most, 2);
    }

    fn holding(harness: &Harness, kind: TaskKind) -> (mpsc::Sender<()>, Receiver<()>) {
        let (release, released) = mpsc::channel::<()>();
        let (started, running) = mpsc::channel();
        harness
            .runner
            .start(harness.repo(), kind, "test".to_string(), move |_| {
                started.send(()).unwrap();
                let _ = released.recv_timeout(WAIT);
                Ok(())
            });
        running.recv_timeout(WAIT).unwrap();
        (release, running)
    }

    fn turn(
        harness: &Harness,
        take: impl FnOnce(&TaskContext, Box<dyn FnOnce() + '_>) -> Option<()> + Send + 'static,
    ) -> (TaskId, Receiver<bool>) {
        let (send, reached) = mpsc::channel();
        let id = harness.runner.start(
            harness.repo(),
            TaskKind::Read,
            "test".to_string(),
            move |ctx| {
                let sender = send.clone();
                let ran = take(ctx, Box::new(move || sender.send(true).unwrap()));
                if ran.is_none() {
                    send.send(false).unwrap();
                }
                Ok(())
            },
        );
        (id, reached)
    }

    // A piece that takes a while and cannot be cut off, like what goes from stashing to restoring.
    fn writing_for(harness: &Harness, work: Duration) -> (Receiver<()>, Receiver<bool>) {
        let (entered, inside) = mpsc::channel();
        let (send, finished) = mpsc::channel();
        harness.runner.start(
            harness.repo(),
            TaskKind::Read,
            "test".to_string(),
            move |ctx| {
                let done = ctx.exclusive(|| {
                    entered.send(()).unwrap();
                    thread::sleep(work);
                    ctx.check_cancelled().is_err()
                });
                send.send(done.unwrap_or(false)).unwrap();
                Ok(())
            },
        );
        (inside, finished)
    }

    #[test]
    fn on_quit_the_writer_is_cancelled_and_waited_for() {
        let harness = Harness::new();
        let (inside, finished) = writing_for(&harness, Duration::from_millis(200));
        inside.recv_timeout(WAIT).unwrap();

        assert!(harness.runner.finish_writes(WAIT));

        assert_eq!(
            finished.try_recv(),
            Ok(true),
            "it already finished and saw the cancellation"
        );
    }

    #[test]
    fn on_quit_a_write_task_is_also_waited_for() {
        let harness = Harness::new();
        let (entered, inside) = mpsc::channel();
        let (send, finished) = mpsc::channel();
        harness.runner.start(
            harness.repo(),
            TaskKind::Write,
            "test".to_string(),
            move |ctx| {
                entered.send(()).unwrap();
                thread::sleep(Duration::from_millis(200));
                send.send(ctx.check_cancelled().is_err()).unwrap();
                Ok(())
            },
        );
        inside.recv_timeout(WAIT).unwrap();

        assert!(harness.runner.finish_writes(WAIT));

        assert_eq!(finished.try_recv(), Ok(true));
    }

    #[test]
    fn on_quit_the_wait_has_a_limit() {
        let harness = Harness::new();
        let (inside, finished) = writing_for(&harness, Duration::from_secs(2));
        inside.recv_timeout(WAIT).unwrap();

        assert!(!harness.runner.finish_writes(Duration::from_millis(50)));

        assert!(finished.try_recv().is_err());
        finished.recv_timeout(WAIT).unwrap();
    }

    #[test]
    fn on_quit_nobody_starts_writing() {
        let harness = Harness::new();

        assert!(harness.runner.finish_writes(WAIT));

        let (inside, finished) = writing_for(&harness, Duration::ZERO);
        assert_eq!(finished.recv_timeout(WAIT), Ok(false));
        assert!(inside.try_recv().is_err());
    }

    #[test]
    fn on_quit_a_read_is_not_waited_for() {
        let harness = Harness::new();
        let (release, _running) = holding(&harness, TaskKind::Read);

        assert!(harness.runner.finish_writes(Duration::from_millis(50)));

        release.send(()).unwrap();
    }

    #[test]
    fn a_task_piece_shares_its_turn_with_the_tasks_of_that_remote() {
        let harness = Harness::new();
        let (release, _running) = holding(&harness, TaskKind::Remote("origin".into()));

        let (_, reached) = turn(&harness, |ctx, work| ctx.on_remote("origin", work));

        assert!(reached.recv_timeout(Duration::from_millis(200)).is_err());
        release.send(()).unwrap();
        assert_eq!(reached.recv_timeout(WAIT), Ok(true));
    }

    #[test]
    fn a_task_piece_shares_its_turn_with_the_writes() {
        let harness = Harness::new();
        let (release, _running) = holding(&harness, TaskKind::Write);

        let (_, reached) = turn(&harness, |ctx, work| ctx.exclusive(work));

        assert!(reached.recv_timeout(Duration::from_millis(200)).is_err());
        release.send(()).unwrap();
        assert_eq!(reached.recv_timeout(WAIT), Ok(true));
    }

    #[test]
    fn cancelling_while_a_piece_waits_for_its_turn_does_not_run_it() {
        let harness = Harness::new();
        let (release, _running) = holding(&harness, TaskKind::Write);

        let (id, reached) = turn(&harness, |ctx, work| ctx.exclusive(work));
        harness.runner.cancel(id);

        assert_eq!(reached.recv_timeout(WAIT), Ok(false));
        release.send(()).unwrap();
    }

    #[test]
    fn remote_task_does_not_block_a_commit() {
        let harness = Harness::new();
        let (release, held) = mpsc::channel::<()>();
        let (started, ready) = mpsc::channel();
        let id = harness.runner.start(
            harness.repo(),
            TaskKind::Remote("origin".into()),
            "fetch".to_string(),
            move |_| {
                started.send(()).unwrap();
                held.recv_timeout(WAIT).unwrap();
                Ok(())
            },
        );
        ready.recv_timeout(WAIT).unwrap();

        let free = harness.locks.try_run(harness.repo(), Duration::ZERO, || ());

        assert!(free.is_some());
        release.send(()).unwrap();
        harness.until_finished(id);
    }

    #[test]
    fn state_arrives_through_events() {
        let harness = Harness::new();
        let done = harness
            .runner
            .start(harness.repo(), TaskKind::Read, "good".to_string(), |_| {
                Ok(())
            });
        let failed =
            harness
                .runner
                .start(harness.repo(), TaskKind::Read, "bad".to_string(), |_| {
                    Err::<(), _>(GitError::Io("no disk".to_string()))
                });
        let cancelled = harness.runner.start(
            harness.repo(),
            TaskKind::Read,
            "cancels".to_string(),
            |_| Err::<(), _>(GitError::Cancelled),
        );

        let done = harness.until_finished(done);
        let failed = harness.until_finished(failed);
        let cancelled = harness.until_finished(cancelled);

        assert_eq!(done[0].state, TaskState::Running);
        assert_eq!(done[0].kind, None);
        assert_eq!(done[0].title, "good");
        assert_eq!(done.last().unwrap().state, TaskState::Done);
        let failed = failed.last().unwrap();
        assert_eq!(
            (failed.state, failed.text.as_str()),
            (TaskState::Failed, "no disk")
        );
        assert_eq!(cancelled.last().unwrap().state, TaskState::Cancelled);
    }

    #[test]
    fn the_failure_carries_the_error_type() {
        let harness = Harness::new();
        let errors = [
            (GitError::NotInstalled, "git_not_found"),
            (GitError::Io("disk".to_string()), "failed"),
            (GitError::IndexLocked, "failed"),
            (GitError::Outdated, "outdated"),
            (GitError::Cancelled, "cancelled"),
        ];
        for (error, kind) in errors {
            let id = harness.runner.start(
                harness.repo(),
                TaskKind::Read,
                "fails".to_string(),
                move |_| Err::<(), _>(error),
            );
            let last = harness.until_finished(id).pop().unwrap();
            assert_eq!(last.kind, Some(kind));
            assert_eq!(last.value, None);
        }
    }

    #[test]
    fn the_result_arrives_in_the_final_event() {
        #[derive(Serialize)]
        struct Updated {
            files: u32,
            names: Vec<&'static str>,
        }
        let harness = Harness::new();
        let id = harness
            .runner
            .start(harness.repo(), TaskKind::Read, "update".to_string(), |_| {
                Ok(Updated {
                    files: 2,
                    names: vec!["a", "b"],
                })
            });

        let events = harness.until_finished(id);

        let last = events.last().unwrap();
        assert_eq!(last.state, TaskState::Done);
        assert_eq!(
            last.value,
            Some(serde_json::json!({ "files": 2, "names": ["a", "b"] }))
        );
        assert_eq!(last.kind, None);
        assert!(events[..events.len() - 1]
            .iter()
            .all(|event| event.value.is_none()));
    }

    // A partial result on a failure is delivered as `Ok`: the value arrives and the state is `done`.
    #[test]
    fn a_partial_result_travels_as_ok_even_if_git_failed() {
        let harness = Harness::fake("echo 'rejected' >&2\nexit 1");
        let id = harness
            .runner
            .start(harness.repo(), TaskKind::Read, "push".to_string(), |ctx| {
                let rejected = ctx.git(&["push"]).is_err();
                Ok(serde_json::json!({ "rejected": rejected }))
            });

        let last = harness.until_finished(id).pop().unwrap();

        assert_eq!(last.state, TaskState::Done);
        assert_eq!(last.value, Some(serde_json::json!({ "rejected": true })));
    }

    #[test]
    fn a_task_failure_hides_the_credentials() {
        let harness = Harness::fake(
            "echo \"fatal: unable to access 'https://u:pass@host/r/': 403\" >&2\nexit 128",
        );
        let (id, _) = harness.git(
            TaskKind::Remote("origin".into()),
            &["fetch", "https://u:pass@host/r"],
        );

        let last = harness.until_finished(id).pop().unwrap();

        assert_eq!(last.state, TaskState::Failed);
        assert!(!last.text.contains("pass"), "{}", last.text);
        assert!(last.text.contains("https://u:***@host/r"), "{}", last.text);
    }

    #[test]
    fn cancelling_a_task_that_waits_for_the_lock_does_not_run_it() {
        let harness = Harness::new();
        let (release, held) = mpsc::channel::<()>();
        let (started, ready) = mpsc::channel();
        let first = harness.runner.start(
            harness.repo(),
            TaskKind::Write,
            "first".to_string(),
            move |_| {
                started.send(()).unwrap();
                held.recv_timeout(WAIT).unwrap();
                Ok(())
            },
        );
        ready.recv_timeout(WAIT).unwrap();
        let (ran, did_run) = mpsc::channel();
        let second = harness.runner.start(
            harness.repo(),
            TaskKind::Write,
            "second".to_string(),
            move |_| {
                ran.send(()).unwrap();
                Ok(())
            },
        );

        harness.runner.cancel(second);

        let events = harness.until_finished(second);
        assert_eq!(events.last().unwrap().state, TaskState::Cancelled);
        assert!(did_run.try_recv().is_err());
        release.send(()).unwrap();
        harness.until_finished(first);
    }

    // Waits for the fake to have started: cancelling earlier would prove nothing.
    fn wait_for_file(path: &Path) {
        let deadline = Instant::now() + WAIT;
        while !path.exists() {
            assert!(Instant::now() < deadline, "the fake did not start");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn alive(pid: i32) -> bool {
        unsafe { libc::kill(pid, 0) == 0 }
    }

    fn wait_until_dead(pid: i32) -> bool {
        let deadline = Instant::now() + Duration::from_secs(2);
        while alive(pid) {
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(10));
        }
        true
    }

    #[test]
    fn cancelling_kills_the_child() {
        let marks = TempDir::new().unwrap();
        let ready = marks.path().join("ready");
        let harness = Harness::fake(&format!("touch '{}'\nsleep 30", ready.display()));
        let (id, result) = harness.git(TaskKind::Read, &["fetch"]);
        wait_for_file(&ready);

        let started = Instant::now();
        harness.runner.cancel(id);

        let result = result.recv_timeout(WAIT).unwrap();
        assert_eq!(result, Err(GitError::Cancelled));
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(
            harness.until_finished(id).last().unwrap().state,
            TaskState::Cancelled
        );
    }

    #[test]
    fn cancelling_kills_the_grandchild() {
        let marks = TempDir::new().unwrap();
        let ready = marks.path().join("ready");
        let grandchild = marks.path().join("grandchild");
        let harness = Harness::fake(&format!(
            "sleep 30 &\necho $! > '{}'\ntouch '{}'\nwait",
            grandchild.display(),
            ready.display()
        ));
        let (id, result) = harness.git(TaskKind::Read, &["fetch"]);
        wait_for_file(&ready);
        let pid: i32 = fs::read_to_string(&grandchild)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(alive(pid));

        harness.runner.cancel(id);

        assert_eq!(result.recv_timeout(WAIT).unwrap(), Err(GitError::Cancelled));
        assert!(wait_until_dead(pid), "the grandchild is still alive");
    }

    #[test]
    fn cancelling_escalates_to_sigkill() {
        let marks = TempDir::new().unwrap();
        let ready = marks.path().join("ready");
        let grace = Duration::from_millis(400);
        let harness = Harness::fake_with_grace(
            &format!(
                "trap '' TERM\ntouch '{}'\nwhile true; do sleep 0.05; done",
                ready.display()
            ),
            grace,
        );
        let (id, result) = harness.git(TaskKind::Read, &["fetch"]);
        wait_for_file(&ready);

        let started = Instant::now();
        harness.runner.cancel(id);

        assert_eq!(result.recv_timeout(WAIT).unwrap(), Err(GitError::Cancelled));
        let elapsed = started.elapsed();
        assert!(elapsed >= grace, "died before the deadline: {elapsed:?}");
        assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
    }

    #[test]
    fn progress_reads_lines_with_carriage_return() {
        let harness = Harness::fake(
            "printf 'Receiving objects:  45%%\\r' >&2\nprintf 'Receiving objects: 100%%\\r' >&2\nprintf 'done\\n' >&2",
        );
        let (id, result) = harness.git(TaskKind::Remote("origin".into()), &["push", "--progress"]);

        let output = result.recv_timeout(WAIT).unwrap().unwrap();
        let events = harness.until_finished(id);

        let fractions: Vec<_> = events.iter().filter_map(|event| event.fraction).collect();
        assert_eq!(fractions, vec![0.45, 1.0]);
        assert!(events
            .iter()
            .any(|event| event.text == "Receiving objects:  45%"));
        assert_eq!(output.stderr, "done\n");
        let lines = harness.console.lines();
        assert!(
            lines.iter().all(|line| !line.text.contains("Receiving")),
            "{lines:?}"
        );
        assert!(lines.iter().any(|line| line.text == "done"));
    }

    #[test]
    fn the_output_is_returned_whole() {
        let harness = Harness::fake("printf 'one\\n\\ntwo' \necho warning >&2");
        let (_, result) = harness.git(TaskKind::Read, &["rev-list"]);

        let output = result.recv_timeout(WAIT).unwrap().unwrap();

        assert_eq!(output.stdout, "one\n\ntwo");
        assert_eq!(output.stderr, "warning\n");
    }

    #[test]
    fn a_failure_carries_the_command_and_the_error() {
        let harness = Harness::fake("echo 'fatal: no' >&2\nexit 3");
        let (_, result) = harness.git(TaskKind::Read, &["fetch", "origin"]);

        let error = result.recv_timeout(WAIT).unwrap().unwrap_err();

        assert_eq!(
            error,
            GitError::Failed {
                args: vec!["fetch".to_string(), "origin".to_string()],
                code: Some(3),
                stdout: String::new(),
                stderr: "fatal: no\n".to_string(),
            }
        );
    }

    #[test]
    fn the_command_carries_intellijs_options() {
        let marks = TempDir::new().unwrap();
        let log = marks.path().join("args");
        let harness = Harness::fake(&format!("echo \"$@\" > '{}'", log.display()));
        let (_, result) = harness.git(TaskKind::Read, &["rev-list", "HEAD"]);
        result.recv_timeout(WAIT).unwrap().unwrap();

        assert_eq!(
            fs::read_to_string(log).unwrap(),
            "-c core.quotepath=false -c log.showSignature=false rev-list HEAD\n"
        );
    }

    fn logged_args(harness: &Harness, log: &Path, args: &[&str]) -> String {
        let (_, result) = harness.git(TaskKind::Read, args);
        result.recv_timeout(WAIT).unwrap().unwrap();
        fs::read_to_string(log).unwrap()
    }

    #[test]
    fn helper_off_adds_an_empty_config() {
        let marks = TempDir::new().unwrap();
        let log = marks.path().join("args");
        let harness = Harness::fake(&format!("echo \"$@\" > '{}'", log.display()));
        harness.runner.set_credential_helper(false);

        assert_eq!(
            logged_args(&harness, &log, &["fetch", "origin"]),
            "-c credential.helper= -c core.quotepath=false -c log.showSignature=false fetch origin\n"
        );
        assert_eq!(
            logged_args(&harness, &log, &["config", "--get", "user.name"]),
            "-c core.quotepath=false -c log.showSignature=false config --get user.name\n"
        );
        assert_eq!(
            logged_args(&harness, &log, &["-c", "a=b", "config", "--list"]),
            "-c core.quotepath=false -c log.showSignature=false -c a=b config --list\n"
        );
    }

    #[test]
    fn helper_off_shows_in_the_console() {
        let harness = Harness::fake("exit 0");
        harness.runner.set_credential_helper(false);
        let (_, result) = harness.git(TaskKind::Read, &["push"]);
        result.recv_timeout(WAIT).unwrap().unwrap();

        let lines = harness.console.lines();
        assert!(
            lines[0].text.ends_with(
                "git -c credential.helper= -c core.quotepath=false -c log.showSignature=false push"
            ),
            "{lines:?}"
        );
    }

    #[test]
    fn quiet_task_does_not_ask_nor_show() {
        let harness = Harness::fake(r#"echo "$GIT_ASKPASS|$SSH_ASKPASS|$GCM_INTERACTIVE""#);
        harness.runner.use_askpass(ClientEnv {
            program: PathBuf::from("/the/app"),
            socket: PathBuf::from("/socket"),
            token: "token".to_string(),
        });

        let output = harness
            .runner
            .quiet(harness.repo(), Duration::from_secs(5), |ctx| {
                ctx.git(&["ls-remote"])
            })
            .unwrap();

        assert_eq!(
            output.stdout.trim(),
            "/usr/bin/false|/usr/bin/false|never",
            "no question reaches the app"
        );
        assert!(harness.events.try_recv().is_err(), "nothing in the bar");
        assert!(harness.console.lines().is_empty());
    }

    #[test]
    fn a_normal_task_does_use_askpass() {
        let harness = Harness::fake(r#"echo "$GIT_ASKPASS""#);
        harness.runner.use_askpass(ClientEnv {
            program: PathBuf::from("/the/app"),
            socket: PathBuf::from("/socket"),
            token: "token".to_string(),
        });
        let repo = harness.repo().to_path_buf();

        let output =
            crate::test_support::in_task(&harness.runner, &repo, |ctx| ctx.git(&["fetch"]))
                .unwrap();

        assert_eq!(output.stdout.trim(), "/the/app");
    }

    #[test]
    fn quiet_task_is_cut_off_at_the_limit() {
        let harness = Harness::fake_with_grace("exec sleep 30", Duration::from_millis(100));
        let started = Instant::now();

        let result = harness
            .runner
            .quiet(harness.repo(), Duration::from_millis(100), |ctx| {
                ctx.git(&["ls-remote"])
            });

        assert_eq!(result.unwrap_err(), GitError::Cancelled);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn without_askpass_the_console_reports_it() {
        let harness = Harness::new();

        harness
            .runner
            .askpass_unavailable(&io::Error::other("no room"));

        let lines = harness.console.lines();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(!lines[0].command);
        assert!(lines[0].text.contains("no room"), "{lines:?}");
    }

    #[test]
    fn helper_on_adds_nothing() {
        let marks = TempDir::new().unwrap();
        let log = marks.path().join("args");
        let harness = Harness::fake(&format!("echo \"$@\" > '{}'", log.display()));

        assert_eq!(
            logged_args(&harness, &log, &["fetch", "origin"]),
            "-c core.quotepath=false -c log.showSignature=false fetch origin\n"
        );
    }

    #[test]
    fn console_shows_only_commands_that_write() {
        let remote = TempRepo::new();
        remote.commit("a.txt", b"a", "one");
        let harness = Harness::new();
        let remote_path = remote.path().display().to_string();
        let repo = TempRepo::new();
        repo.git(&["remote", "add", "origin", &remote_path]);
        let (send, result) = mpsc::channel();
        let id = harness.runner.start(
            repo.path(),
            TaskKind::Remote("origin".into()),
            "fetch".to_string(),
            move |ctx| {
                ctx.git(&["rev-list", "--count", "--all"])?;
                ctx.git(&["stash", "list"])?;
                send.send(ctx.git_loud(&["fetch", "origin"])).unwrap();
                // Without `git_loud`, fetch is a read and is not printed.
                ctx.git(&["fetch", "origin"])?;
                Ok(())
            },
        );
        result.recv_timeout(WAIT).unwrap().unwrap();
        harness.until_finished(id);

        let folder = repo
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let commands: Vec<_> = harness
            .console
            .lines()
            .into_iter()
            .filter(|line| line.command)
            .collect();
        assert_eq!(commands.len(), 1, "{commands:?}");
        assert_eq!(
            commands[0].text,
            format!(
                "[{folder}] git -c core.quotepath=false -c log.showSignature=false fetch origin"
            )
        );
        let all = harness.console.lines();
        assert!(
            all.iter().all(|line| !line.text.contains("rev-list")),
            "{all:?}"
        );
        // The output of fetch is also shown.
        assert!(
            all.iter()
                .any(|line| !line.command && line.text.contains("origin/")),
            "{all:?}"
        );
    }
}
