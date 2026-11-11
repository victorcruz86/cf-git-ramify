// The git editor. Git launches `GIT_EDITOR` (messages) and `GIT_SEQUENCE_EDITOR` (a rebase's list)
// with a file and waits for the process to leave it written. That process is this very binary
// (`client_main`), which asks the open app for the text over a Unix socket, as `askpass` does: git
// never waits for a terminal. Ported from `GitHandlerRebaseEditorManager`, `GitRebaseEditorService` and
// `GitInteractiveRebaseEditorHandler`.
use std::collections::HashMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions, Permissions};
use std::io::{self, Read, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use serde::Serialize;
use tempfile::TempDir;

use crate::askpass::{
    random_token, same_token, socket_folder, MAX_MESSAGE_BYTES, REFUSING_PROGRAM, REQUEST_TIMEOUT,
    SOCKET_FOLDER_RANDOM_CHARS, SOCKET_NAME, TOKEN_BYTES,
};

const SOCKET_VARIABLE: &str = "RAMIFY_EDITOR_SOCKET";
const TOKEN_VARIABLE: &str = "RAMIFY_EDITOR_TOKEN";
const SOCKET_FOLDER_PREFIX: &str = "ramify-editor-";
const TOKEN_CHARS: usize = TOKEN_BYTES * 2;
// Git launches the same binary as askpass and as editor, and a task's environment carries the
// variables of both: this flag, which goes before the file, says which of the two it is.
const CLIENT_FLAG: &str = "--ramify-editor";
const TODO_FILE: &str = "git-rebase-todo";
// `GitUtil.COMMENT_CHAR`: neither the user nor a template uses it, so whatever starts with it is
// git's help and nothing else.
const COMMENT_CHAR: char = '\u{1}';
// What the server answers if it did the work; closing without answering is the refusal.
const DONE: u8 = 0;
// How often to check whether the server closed while waiting for an answer from the window.
const POLL: Duration = Duration::from_millis(20);

// `GitImpl.REBASE_CONFIG_PARAMS`. With it, the help lines of the rebase and of the messages arrive
// marked and can be removed without touching the user's, which start with `#`. Also, the todo's
// commands arrive in full even if the user has `rebase.abbreviateCommands`: the model does not read `p`.
pub const REBASE_CONFIG: [&str; 4] = [
    "-c",
    "core.commentChar=\u{1}",
    "-c",
    "rebase.abbreviateCommands=false",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EditKind {
    Todo,
    Message,
}

impl EditKind {
    // `COMMIT_EDITMSG`, `MERGE_MSG`, `SQUASH_MSG`... are messages.
    pub fn of(file: &Path) -> Self {
        if file.file_name() == Some(OsStr::new(TODO_FILE)) {
            Self::Todo
        } else {
            Self::Message
        }
    }
}

// How the window shows a request: the free-text editor (`GitUnstructuredEditor`), the
// `Rebasing Commits` dialog with the list already read, or the `noop` question.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Form {
    Text,
    Commits,
    Noop,
}

// What reaches the window. With `Commits`, `text` is the list in JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Prompt {
    pub kind: EditKind,
    pub form: Form,
    pub text: String,
}

impl Prompt {
    // Without git's help and without the blank lines at the start, as
    // `loadFileAndShowInSimpleEditor`.
    pub fn text(kind: EditKind, text: &str) -> Self {
        Self {
            kind,
            form: Form::Text,
            text: strip_comments(text).trim_start().to_string(),
        }
    }
}

// `GitImplBase.ignoreComments`.
pub fn strip_comments(text: &str) -> String {
    text.split_inclusive('\n')
        .filter(|line| !line.starts_with(COMMENT_CHAR))
        .collect()
}

// The request the server has open while a handler decides: `Ask` uses it to wait for the
// window's answer, and the server withdraws it if the editor leaves first.
pub struct Request<'a> {
    id: u64,
    // The project window of the registration: the one that is asked and the only one that may answer.
    owner: &'a str,
    // Behind a lock only so it can be shared with the handler's thread.
    answers: &'a Mutex<Receiver<Option<String>>>,
    // Whether it got to be shown: only then is there a dialog to close.
    shown: &'a AtomicBool,
}

pub trait EditorHandler: Send + Sync {
    // The file's new text. `None` cancels: the editor exits with 1 and git gives up.
    fn edit(&self, kind: EditKind, text: &str) -> Option<String>;

    // What the server calls. A handler that waits for someone (like `Ask`) redefines it to
    // wait with `request`: that way, if the editor dies (the task was cancelled) or the
    // registration is dropped, the wait ends. The others do not need to know about it.
    fn edit_request(&self, _request: &Request, kind: EditKind, text: &str) -> Option<String> {
        self.edit(kind, text)
    }
}

// The text as is: for when all that matters is that git does not stop.
pub struct Trivial;

impl EditorHandler for Trivial {
    fn edit(&self, _: EditKind, text: &str) -> Option<String> {
        Some(text.to_string())
    }
}

// Asks the project window `owner`. The text is shown without git's help and without the blank
// lines at the start, as `loadFileAndShowInSimpleEditor`. Without a server, there is no one to
// ask and it is cancelled.
pub struct Ask {
    handle: Option<Handle>,
    owner: String,
}

impl Ask {
    pub fn new(handle: Option<Handle>, owner: String) -> Self {
        Self { handle, owner }
    }

    // With `request`, the one the server has open; without it (a caller that does not go through
    // the server), one of its own, with no one to withdraw it if the editor leaves.
    pub fn prompt(&self, request: Option<&Request>, prompt: Prompt) -> Option<String> {
        let shared = &self.handle.as_ref()?.shared;
        if let Some(request) = request {
            return shared.ask(request, prompt);
        }
        let (id, answers) = shared.open(&self.owner);
        let shown = AtomicBool::new(false);
        let request = Request {
            id,
            owner: &self.owner,
            answers: &answers,
            shown: &shown,
        };
        let answer = shared.ask(&request, prompt);
        shared.remove(id);
        answer
    }
}

impl EditorHandler for Ask {
    fn edit(&self, kind: EditKind, text: &str) -> Option<String> {
        self.prompt(None, Prompt::text(kind, text))
    }

    fn edit_request(&self, request: &Request, kind: EditKind, text: &str) -> Option<String> {
        self.prompt(Some(request), Prompt::text(kind, text))
    }
}

// The project window a request belongs to, its id, and what it asks.
type OnRequest = Box<dyn Fn(&str, u64, Prompt) -> bool + Send + Sync>;
type OnDismissed = Box<dyn Fn(&str, u64) + Send + Sync>;

// A request waiting for its window's answer.
struct Pending {
    owner: String,
    answer: Sender<Option<String>>,
}

struct Entry {
    // The project window of the task that registered.
    owner: String,
    handler: Arc<dyn EditorHandler>,
    git_dir: PathBuf,
    // Requests in which git is stopped waiting for the editor.
    waiting: AtomicUsize,
}

struct Shared {
    program: PathBuf,
    socket: PathBuf,
    closed: AtomicBool,
    next: AtomicU64,
    registered: Mutex<HashMap<String, Arc<Entry>>>,
    pending: Mutex<HashMap<u64, Pending>>,
    on_request: OnRequest,
    on_dismissed: OnDismissed,
}

impl Shared {
    // The token is not looked up by equality: timing must not say how many bytes it got right.
    fn entry(&self, received: &[u8]) -> Option<(String, Arc<Entry>)> {
        let registered = self
            .registered
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        registered.iter().fold(None, |found, (token, entry)| {
            // `same_token` only compares up to the shorter one: an empty token would accept any.
            let same = received.len() == token.len() && same_token(received, token.as_bytes());
            found.or(same.then(|| (token.clone(), entry.clone())))
        })
    }

    fn is_registered(&self, token: &str) -> bool {
        self.registered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(token)
    }

    fn remove(&self, id: u64) -> Option<Pending> {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id)
    }

    // Opens a request of the project window `owner`: until it is withdrawn, `answer` can respond to it.
    fn open(&self, owner: &str) -> (u64, Mutex<Receiver<Option<String>>>) {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let (send, answers) = mpsc::channel();
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                id,
                Pending {
                    owner: owner.to_string(),
                    answer: send,
                },
            );
        (id, Mutex::new(answers))
    }

    // `false` only if the request belongs to another window: then it keeps waiting for its own.
    fn answer(&self, owner: &str, id: u64, text: Option<String>) -> bool {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        match pending.get(&id) {
            None => true,
            Some(waiting) if waiting.owner != owner => false,
            Some(_) => {
                if let Some(waiting) = pending.remove(&id) {
                    let _ = waiting.answer.send(text);
                }
                true
            }
        }
    }

    // Waits for the window to answer. If no one can receive the request, or the server closes,
    // there will never be an answer: it is refused. Withdrawing the request (`remove`) also wakes
    // the wait, with `None`.
    fn ask(&self, request: &Request, prompt: Prompt) -> Option<String> {
        if self.closed.load(Ordering::SeqCst) {
            return None;
        }
        request.shown.store(true, Ordering::SeqCst);
        if !(self.on_request)(request.owner, request.id, prompt) {
            request.shown.store(false, Ordering::SeqCst);
            return None;
        }
        let answers = request
            .answers
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        answers.recv().ok().flatten()
    }
}

// What a task needs from the server: to register and to ask. It can be cloned; it does not keep
// the socket alive.
#[derive(Clone)]
pub struct Handle {
    shared: Arc<Shared>,
}

impl Handle {
    // The handler only sees files inside `git_dir`, the git directory of the task's repository,
    // and its requests go to the project window `owner`. The registration is dropped when it is
    // released.
    pub fn register(
        &self,
        owner: String,
        handler: Arc<dyn EditorHandler>,
        git_dir: PathBuf,
    ) -> Registration {
        // Without entropy there is no token: the registration stays unregistered and the editor exits with 1.
        let token = random_token().unwrap_or_default();
        if !token.is_empty() {
            self.shared
                .registered
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(
                    token.clone(),
                    Arc::new(Entry {
                        owner,
                        handler,
                        git_dir,
                        waiting: AtomicUsize::new(0),
                    }),
                );
        }
        Registration {
            token,
            shared: self.shared.clone(),
        }
    }

    // `None` cancels. Only the project window `owner` the request belongs to may answer it:
    // `false` if it belongs to another, and then it keeps waiting. If no one is waiting anymore,
    // nothing happens.
    pub fn answer(&self, owner: &str, id: u64, text: Option<String>) -> bool {
        self.shared.answer(owner, id, text)
    }
}

// Without `Debug`: the token must not be able to end up in a message.
pub struct Registration {
    token: String,
    shared: Arc<Shared>,
}

impl Registration {
    pub fn apply(&self, command: &mut Command) {
        // Git launches them with `sh`: the program goes between quotes, since it may have spaces.
        let mut editor = posix_quote(&self.shared.program);
        editor.push(" ");
        editor.push(CLIENT_FLAG);
        command
            .env("GIT_EDITOR", &editor)
            .env("GIT_SEQUENCE_EDITOR", &editor)
            .env(SOCKET_VARIABLE, &self.shared.socket)
            .env(TOKEN_VARIABLE, &self.token);
    }
}

impl Registration {
    // The task is cancelled: the editor no longer serves anyone with this registration. Whatever is
    // waiting is withdrawn, the editor exits with 1 and git gives up on its own. It returns whether git was
    // stopped in the editor, and therefore is going to exit without needing a signal.
    pub fn withdraw(&self) -> bool {
        let entry = self
            .shared
            .registered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.token);
        entry.is_some_and(|entry| entry.waiting.load(Ordering::SeqCst) > 0)
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.shared
            .registered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.token);
    }
}

// Without a server, an editor that answers nothing and exits with 1: without this git would open `vi` without a terminal.
pub fn refuse(command: &mut Command) {
    command
        .env("GIT_EDITOR", REFUSING_PROGRAM)
        .env("GIT_SEQUENCE_EDITOR", REFUSING_PROGRAM);
}

// `CommandLineUtil.posixQuote`.
fn posix_quote(text: &Path) -> OsString {
    let mut quoted = vec![b'\''];
    for &byte in text.as_os_str().as_bytes() {
        if byte == b'\'' {
            quoted.extend_from_slice(b"'\\''");
        } else {
            quoted.push(byte);
        }
    }
    quoted.push(b'\'');
    OsString::from_vec(quoted)
}

pub struct Server {
    handle: Handle,
    // Dropping it deletes the socket.
    _folder: TempDir,
}

impl Server {
    // `on_request` returns whether the request reached someone: if not, no one would ever answer and it
    // is refused. The socket lives in a `0700` directory, the owner's only.
    // `on_dismissed` reports that whoever asked is no longer waiting (the task was cancelled and git killed the
    // editor, or it finished): that way the dialog does not stay open for no one.
    pub fn start(
        on_request: impl Fn(&str, u64, Prompt) -> bool + Send + Sync + 'static,
        on_dismissed: impl Fn(&str, u64) + Send + Sync + 'static,
    ) -> io::Result<Self> {
        Self::start_with_program(env::current_exe()?, on_request, on_dismissed)
    }

    // The integration tests launch the app's binary, not the test's.
    pub fn start_with_program(
        program: PathBuf,
        on_request: impl Fn(&str, u64, Prompt) -> bool + Send + Sync + 'static,
        on_dismissed: impl Fn(&str, u64) + Send + Sync + 'static,
    ) -> io::Result<Self> {
        let folder = tempfile::Builder::new()
            .prefix(SOCKET_FOLDER_PREFIX)
            .rand_bytes(SOCKET_FOLDER_RANDOM_CHARS)
            .permissions(Permissions::from_mode(0o700))
            .tempdir_in(socket_folder(&env::temp_dir()))?;
        let socket = folder.path().join(SOCKET_NAME);
        let listener = UnixListener::bind(&socket)?;
        let shared = Arc::new(Shared {
            program,
            socket,
            closed: AtomicBool::new(false),
            next: AtomicU64::new(1),
            registered: Mutex::default(),
            pending: Mutex::default(),
            on_request: Box::new(on_request),
            on_dismissed: Box::new(on_dismissed),
        });
        let accepting = shared.clone();
        thread::spawn(move || accept(listener, accepting));
        Ok(Self {
            handle: Handle { shared },
            _folder: folder,
        })
    }

    pub fn handle(&self) -> Handle {
        self.handle.clone()
    }

    pub fn register(
        &self,
        owner: String,
        handler: Arc<dyn EditorHandler>,
        git_dir: PathBuf,
    ) -> Registration {
        self.handle.register(owner, handler, git_dir)
    }

    pub fn answer(&self, owner: &str, id: u64, text: Option<String>) -> bool {
        self.handle.answer(owner, id, text)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let shared = &self.handle.shared;
        shared.closed.store(true, Ordering::SeqCst);
        // Whoever is waiting for an answer will not get one: dropping the channels wakes them.
        shared
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        // Wakes `accept`, which is blocked waiting for a connection.
        let _ = UnixStream::connect(&shared.socket);
    }
}

fn accept(listener: UnixListener, shared: Arc<Shared>) {
    for stream in listener.incoming() {
        if shared.closed.load(Ordering::SeqCst) {
            return;
        }
        match stream {
            Ok(stream) => {
                let shared = shared.clone();
                thread::spawn(move || serve(stream, &shared));
            }
            Err(_) => thread::sleep(POLL),
        }
    }
}

// `<length, 4 bytes><token><path>`.
fn read_request(stream: &mut UnixStream) -> io::Result<Option<(Vec<u8>, PathBuf)>> {
    let mut length = [0; size_of::<u32>()];
    stream.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if !(TOKEN_CHARS..=MAX_MESSAGE_BYTES).contains(&length) {
        return Ok(None);
    }
    let mut message = vec![0; length];
    stream.read_exact(&mut message)?;
    let path = message.split_off(TOKEN_CHARS);
    Ok(Some((message, PathBuf::from(OsString::from_vec(path)))))
}

// With `canonicalize` on both sides: neither `..` nor a link takes the path out of the git directory.
fn inside(git_dir: &Path, path: &Path) -> Option<PathBuf> {
    let file = path.canonicalize().ok()?;
    let dir = git_dir.canonicalize().ok()?;
    (file.starts_with(dir) && file.is_file()).then_some(file)
}

// The editor sends nothing more after the request: reading can only yield the close (git killed it).
fn client_gone(mut stream: &UnixStream) -> bool {
    match stream.read(&mut [0]) {
        Ok(read) => read == 0,
        Err(error) => !matches!(
            error.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
        ),
    }
}

// The handler runs separately while here we watch that the editor is still alive and the registration
// is still registered. If not, the request is withdrawn (which wakes `Ask`) and the window is told
// that the dialog is no longer of use.
fn run_handler(
    stream: &UnixStream,
    shared: &Shared,
    token: &str,
    entry: &Entry,
    kind: EditKind,
    text: &str,
) -> Option<String> {
    let (id, answers) = shared.open(&entry.owner);
    let shown = AtomicBool::new(false);
    let request = Request {
        id,
        owner: &entry.owner,
        answers: &answers,
        shown: &shown,
    };
    entry.waiting.fetch_add(1, Ordering::SeqCst);
    let watching = stream.set_nonblocking(true).is_ok();
    let edited = thread::scope(|scope| {
        let (send, done) = mpsc::channel();
        let request = &request;
        scope.spawn(move || {
            let _ = send.send(entry.handler.edit_request(request, kind, text));
        });
        loop {
            match done.recv_timeout(POLL) {
                Ok(edited) => return edited,
                Err(RecvTimeoutError::Disconnected) => return None,
                Err(RecvTimeoutError::Timeout) => {}
            }
            if !watching || client_gone(stream) || !shared.is_registered(token) {
                shared.remove(id);
                if shown.load(Ordering::SeqCst) {
                    (shared.on_dismissed)(&entry.owner, id);
                }
                // The wait ends with `None`; the scope waits for the handler to exit.
                return None;
            }
        }
    });
    entry.waiting.fetch_sub(1, Ordering::SeqCst);
    shared.remove(id);
    edited
}

// Closing without writing anything is the refusal: a bad token gets no hint.
fn serve(mut stream: UnixStream, shared: &Shared) -> io::Result<()> {
    stream.set_read_timeout(Some(REQUEST_TIMEOUT))?;
    let Some((received, path)) = read_request(&mut stream)? else {
        return Ok(());
    };
    let Some((token, entry)) = shared.entry(&received) else {
        return Ok(());
    };
    let Some(file) = inside(&entry.git_dir, &path) else {
        return Ok(());
    };
    let Ok(text) = String::from_utf8(fs::read(&file)?) else {
        return Ok(());
    };
    let Some(edited) = run_handler(&stream, shared, &token, &entry, EditKind::of(&file), &text)
    else {
        return Ok(());
    };
    // If the task finished while waiting, the file may belong to another operation: it is not touched.
    if !shared.is_registered(&token) {
        return Ok(());
    }
    stream.set_nonblocking(false)?;
    // Without `create`: the file exists because git expects it, and none is invented.
    OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&file)?
        .write_all(edited.as_bytes())?;
    stream.write_all(&[DONE])
}

fn ask_server(socket: &Path, token: &str, file: &Path) -> io::Result<bool> {
    let mut stream = UnixStream::connect(socket)?;
    let path = file.as_os_str().as_bytes();
    let length = u32::try_from(token.len() + path.len()).map_err(io::Error::other)?;
    let mut message = length.to_be_bytes().to_vec();
    message.extend_from_slice(token.as_bytes());
    message.extend_from_slice(path);
    stream.write_all(&message)?;
    let mut reply = [0];
    stream.read_exact(&mut reply)?;
    Ok(reply[0] == DONE)
}

// Git launches the app's binary with the file as an argument. Without the flag and the variables that
// `Registration` sets, it is the normal app (or the askpass client). Exiting with 1 makes git give up.
pub fn client_main() -> Option<i32> {
    let socket = env::var_os(SOCKET_VARIABLE)?;
    let token = env::var(TOKEN_VARIABLE).ok()?;
    let mut args = env::args_os().skip(1);
    if args.next()? != CLIENT_FLAG {
        return None;
    }
    // Git passes the path as it sees it, sometimes relative to where it runs.
    let done = args
        .next()
        .and_then(|file| std::path::absolute(file).ok())
        .and_then(|file| ask_server(Path::new(&socket), &token, &file).ok())
        .unwrap_or(false);
    Some(if done { 0 } else { 1 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::Receiver;

    #[test]
    fn the_kind_comes_from_the_file_name() {
        for (name, kind) in [
            ("/r/.git/rebase-merge/git-rebase-todo", EditKind::Todo),
            ("/r/.git/COMMIT_EDITMSG", EditKind::Message),
            ("/r/.git/MERGE_MSG", EditKind::Message),
            ("/r/.git/SQUASH_MSG", EditKind::Message),
            ("/r/.git/rebase-merge/message", EditKind::Message),
            ("git-rebase-todo.bak", EditKind::Message),
        ] {
            assert_eq!(EditKind::of(Path::new(name)), kind, "{name}");
        }
    }

    #[test]
    fn stripping_comments_uses_the_rebase_character() {
        let text =
            "Subject\n\u{1} Please enter the commit message\n# written by the user\n\u{1}\nend";

        assert_eq!(strip_comments(text), "Subject\n# written by the user\nend");
        assert_eq!(strip_comments(""), "");
        assert_eq!(strip_comments("\u{1}help only\n"), "");
        // The configuration passed to git is the same one that is stripped.
        assert_eq!(
            REBASE_CONFIG,
            [
                "-c",
                "core.commentChar=\u{1}",
                "-c",
                "rebase.abbreviateCommands=false"
            ]
        );
    }

    #[test]
    fn posix_quotes_protect_spaces_and_single_quotes() {
        assert_eq!(
            posix_quote(Path::new("/Apps/my branch/ramify")),
            OsString::from("'/Apps/my branch/ramify'")
        );
        assert_eq!(posix_quote(Path::new("/a'b")), OsString::from("'/a'\\''b'"));
    }

    fn watched_server() -> (Server, Receiver<(u64, EditKind, String)>, Receiver<u64>) {
        let (send, requests) = mpsc::channel();
        let send = Mutex::new(send);
        let (gone, dismissed) = mpsc::channel();
        let gone = Mutex::new(gone);
        let server = Server::start(
            move |_: &str, id, prompt: Prompt| {
                let asked = (id, prompt.kind, prompt.text);
                send.lock().unwrap().send(asked).is_ok()
            },
            move |_: &str, id| gone.lock().unwrap().send(id).unwrap(),
        )
        .unwrap();
        (server, requests, dismissed)
    }

    fn shown_server() -> (Server, Receiver<(u64, EditKind, String)>) {
        let (server, requests, _) = watched_server();
        (server, requests)
    }

    fn send_request(socket: &Path, token: &str, file: &Path) -> UnixStream {
        let mut stream = UnixStream::connect(socket).unwrap();
        let path = file.as_os_str().as_bytes();
        let mut message = ((token.len() + path.len()) as u32).to_be_bytes().to_vec();
        message.extend_from_slice(token.as_bytes());
        message.extend_from_slice(path);
        stream.write_all(&message).unwrap();
        stream
    }

    fn wait_until(what: &str, condition: impl Fn() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(std::time::Instant::now() < deadline, "{what}");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn an_editor_that_leaves_withdraws_the_request_and_notifies_the_window() {
        let (server, requests, dismissed) = watched_server();
        let git_dir = tempfile::tempdir().unwrap();
        let file = git_dir.path().join("COMMIT_EDITMSG");
        fs::write(&file, "text\n").unwrap();
        let registration = server.register(
            "main".to_string(),
            Arc::new(Ask::new(Some(server.handle()), "main".to_string())),
            git_dir.path().to_path_buf(),
        );
        let stream = send_request(&server.handle.shared.socket, &registration.token, &file);
        let (id, ..) = requests.recv_timeout(Duration::from_secs(5)).unwrap();

        drop(stream);

        assert_eq!(dismissed.recv_timeout(Duration::from_secs(5)), Ok(id));
        wait_until("the request is still open", || {
            server.handle.shared.pending.lock().unwrap().is_empty()
        });
        // An answer that arrives late bothers no one and the file does not change.
        server.answer("main", id, Some("late".to_string()));
        assert_eq!(fs::read_to_string(&file).unwrap(), "text\n");
    }

    #[test]
    fn dropping_the_registration_withdraws_the_pending_request() {
        let (server, requests, dismissed) = watched_server();
        let git_dir = tempfile::tempdir().unwrap();
        let file = git_dir.path().join("COMMIT_EDITMSG");
        fs::write(&file, "text\n").unwrap();
        let registration = server.register(
            "main".to_string(),
            Arc::new(Ask::new(Some(server.handle()), "main".to_string())),
            git_dir.path().to_path_buf(),
        );
        let _stream = send_request(&server.handle.shared.socket, &registration.token, &file);
        let (id, ..) = requests.recv_timeout(Duration::from_secs(5)).unwrap();

        drop(registration);

        assert_eq!(dismissed.recv_timeout(Duration::from_secs(5)), Ok(id));
        wait_until("the request is still open", || {
            server.handle.shared.pending.lock().unwrap().is_empty()
        });
    }

    #[test]
    fn the_request_names_its_owner() {
        let (send, requests) = mpsc::channel();
        let send = Mutex::new(send);
        let server = Server::start(
            move |owner: &str, id, _: Prompt| {
                send.lock().unwrap().send((owner.to_string(), id)).is_ok()
            },
            |_: &str, _| {},
        )
        .unwrap();
        let git_dir = tempfile::tempdir().unwrap();
        let file = git_dir.path().join("COMMIT_EDITMSG");
        fs::write(&file, "text\n").unwrap();
        let registration = server.register(
            "main-2".to_string(),
            Arc::new(Ask::new(Some(server.handle()), "main-2".to_string())),
            git_dir.path().to_path_buf(),
        );
        let mut stream = send_request(&server.handle.shared.socket, &registration.token, &file);

        let (owner, id) = requests.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(owner, "main-2");
        // Another window cannot answer it: the request keeps waiting for its own.
        assert!(!server.answer("main", id, Some("wrong\n".to_string())));
        assert!(server.answer("main-2", id, Some("right\n".to_string())));
        let mut reply = [1];
        stream.read_exact(&mut reply).unwrap();
        assert_eq!(reply, [DONE]);
        assert_eq!(fs::read_to_string(&file).unwrap(), "right\n");
    }

    #[test]
    fn an_answered_request_does_not_report_being_withdrawn() {
        let (server, requests, dismissed) = watched_server();
        let git_dir = tempfile::tempdir().unwrap();
        let file = git_dir.path().join("COMMIT_EDITMSG");
        fs::write(&file, "text\n").unwrap();
        let registration = server.register(
            "main".to_string(),
            Arc::new(Ask::new(Some(server.handle()), "main".to_string())),
            git_dir.path().to_path_buf(),
        );
        let mut stream = send_request(&server.handle.shared.socket, &registration.token, &file);
        let (id, ..) = requests.recv_timeout(Duration::from_secs(5)).unwrap();

        server.answer("main", id, Some("new\n".to_string()));

        let mut reply = [1];
        stream.read_exact(&mut reply).unwrap();
        assert_eq!(reply, [DONE]);
        assert_eq!(fs::read_to_string(&file).unwrap(), "new\n");
        assert!(dismissed.recv_timeout(Duration::from_millis(200)).is_err());
    }

    #[test]
    fn ask_shows_the_text_without_git_help_and_returns_the_answer() {
        let (server, requests) = shown_server();
        let ask = Arc::new(Ask::new(Some(server.handle()), "main".to_string()));
        let asking = ask.clone();
        let worker = thread::spawn(move || {
            asking.edit(
                EditKind::Message,
                "\n\nFix\n\n# from the user\n\u{1} Please enter\n",
            )
        });

        let (id, kind, text) = requests.recv_timeout(Duration::from_secs(5)).unwrap();
        server.answer("main", id, Some("Fixed\n".to_string()));

        assert_eq!(kind, EditKind::Message);
        assert_eq!(text, "Fix\n\n# from the user\n");
        assert_eq!(worker.join().unwrap(), Some("Fixed\n".to_string()));
    }

    #[test]
    fn ask_without_a_server_cancels() {
        assert_eq!(
            Ask::new(None, "main".to_string()).edit(EditKind::Todo, "pick a b\n"),
            None
        );
    }

    #[test]
    fn dropping_the_server_wakes_whoever_is_waiting() {
        let (server, requests) = shown_server();
        let ask = Ask::new(Some(server.handle()), "main".to_string());
        let worker = thread::spawn(move || ask.edit(EditKind::Message, "text"));
        requests.recv_timeout(Duration::from_secs(5)).unwrap();

        drop(server);

        assert_eq!(worker.join().unwrap(), None);
    }

    #[test]
    fn a_request_with_no_one_to_answer_is_refused() {
        let server = Server::start(|_: &str, _, _| false, |_: &str, _| {}).unwrap();

        let ask = Ask::new(Some(server.handle()), "main".to_string());

        assert_eq!(ask.edit(EditKind::Message, "text"), None);
    }

    #[test]
    fn dropping_the_registration_unregisters_it() {
        let (server, _) = shown_server();
        let registration = server.register(
            "main".to_string(),
            Arc::new(Trivial),
            PathBuf::from("/r/.git"),
        );
        let token = registration.token.clone();
        assert!(server.handle.shared.is_registered(&token));

        drop(registration);

        assert!(!server.handle.shared.is_registered(&token));
    }

    #[test]
    fn two_registrations_have_different_tokens() {
        let (server, _) = shown_server();
        let first = server.register("main".to_string(), Arc::new(Trivial), PathBuf::from("/a"));
        let second = server.register("main".to_string(), Arc::new(Trivial), PathBuf::from("/b"));

        assert_eq!(first.token.len(), TOKEN_CHARS);
        assert_ne!(first.token, second.token);
        let found = server.handle.shared.entry(first.token.as_bytes()).unwrap();
        assert_eq!(found.1.git_dir, Path::new("/a"));
    }

    #[test]
    fn the_socket_folder_belongs_to_the_owner_only() {
        let (server, _) = shown_server();

        let folder = server.handle.shared.socket.parent().unwrap().to_path_buf();
        let mode = fs::metadata(&folder).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);

        drop(server);
        assert!(!folder.exists());
    }
}
