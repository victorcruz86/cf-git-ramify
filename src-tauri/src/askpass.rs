// Git's credentials. Git and ssh do not talk to the app: they launch another process that returns
// the answer to them through stdout. That process is this very binary (`client_main`), which asks the
// open app through a Unix socket. The answer only travels through that socket: it is neither stored
// nor logged here.
use std::collections::HashMap;
use std::env;
use std::ffi::OsStr;
use std::fs::{File, Permissions};
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use serde::Serialize;
use tempfile::TempDir;

const SOCKET_VARIABLE: &str = "RAMIFY_ASKPASS_SOCKET";
const TOKEN_VARIABLE: &str = "RAMIFY_ASKPASS_TOKEN";
const SOCKET_FOLDER_PREFIX: &str = "ramify-askpass-";
pub(crate) const SOCKET_FOLDER_RANDOM_CHARS: usize = 6;
pub(crate) const SOCKET_NAME: &str = "s";
pub(crate) const TOKEN_BYTES: usize = 32;
pub(crate) const MAX_MESSAGE_BYTES: usize = 64 * 1024;
// `sun_path` has 104 bytes on Darwin, including the final NUL.
const SOCKET_PATH_LIMIT: usize = 103;
const SHORT_TEMP_FOLDER: &str = "/tmp";
// Without a server, the questions must fail: this answers nothing and exits with 1.
pub(crate) const REFUSING_PROGRAM: &str = "/usr/bin/false";
// The client writes as soon as it connects: whoever does not write does not hold a thread.
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
// How often it checks whether there is an answer yet and whether the client is still alive.
const POLL: Duration = Duration::from_millis(20);
// What IntelliJ looks for in `SSHUtil`.
const HOST_KEY_QUESTION: &str = "Are you sure you want to continue connecting";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PromptKind {
    Username { url: String },
    Password { url: String, user: String },
    Passphrase { key: String },
    SshPassword { user: String },
    HostKey { text: String },
}

// The user of `https://me@host/path`. It comes before the last at-sign of the server: the path and
// the user itself may contain more.
fn split_user(url: &str) -> (String, String) {
    let start = url.find("://").map_or(0, |index| index + 3);
    let end = url[start..]
        .find('/')
        .map_or(url.len(), |index| start + index);
    match url[start..end].rfind('@') {
        Some(at) => (
            format!("{}{}", &url[..start], &url[start + at + 1..]),
            url[start..start + at].to_string(),
        ),
        None => (url.to_string(), String::new()),
    }
}

fn passphrase_key(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("Enter passphrase for ")?;
    let key = rest
        .strip_prefix("key ")
        .unwrap_or(rest)
        .strip_suffix(':')?;
    Some(key.trim_matches('\''))
}

// A text that is not recognized may also be a secret (a code, a PIN): it is asked for hidden, with
// the text shown in place of the URL.
pub fn classify(prompt: &str) -> PromptKind {
    let text = prompt.trim();
    if text.contains(HOST_KEY_QUESTION) {
        return PromptKind::HostKey {
            text: text.to_string(),
        };
    }
    if let Some(url) = text
        .strip_prefix("Username for '")
        .and_then(|rest| rest.strip_suffix("':"))
    {
        return PromptKind::Username {
            url: url.to_string(),
        };
    }
    if let Some(url) = text
        .strip_prefix("Password for '")
        .and_then(|rest| rest.strip_suffix("':"))
    {
        let (url, user) = split_user(url);
        return PromptKind::Password { url, user };
    }
    if let Some(key) = passphrase_key(text) {
        return PromptKind::Passphrase {
            key: key.to_string(),
        };
    }
    if let Some(user) = text.strip_suffix("'s password:") {
        return PromptKind::SshPassword {
            user: user.to_string(),
        };
    }
    PromptKind::Password {
        url: text.to_string(),
        user: String::new(),
    }
}

// Without `Debug`: the token must not be able to end up in a message.
#[derive(Clone)]
pub struct ClientEnv {
    pub program: PathBuf,
    pub socket: PathBuf,
    pub token: String,
}

impl ClientEnv {
    pub fn apply(&self, command: &mut Command) {
        use_program(command, &self.program);
        command
            .env(SOCKET_VARIABLE, &self.socket)
            .env(TOKEN_VARIABLE, &self.token);
    }
}

fn use_program(command: &mut Command, program: &Path) {
    command
        .env("GIT_ASKPASS", program)
        .env("SSH_ASKPASS", program)
        .env("SSH_ASKPASS_REQUIRE", "force");
    // Like `GitHandlerAuthenticationManager`: an OpenSSH older than 8.4 does not know
    // `SSH_ASKPASS_REQUIRE` and ignores `SSH_ASKPASS` if there is no `DISPLAY`.
    if env::var_os("DISPLAY").is_none_or(|display| display.is_empty()) {
        command.env("DISPLAY", ":0.0");
    }
}

// Without a server. Without this, ssh would ask through `/dev/tty`, and that would stop the child forever.
pub fn refuse_prompts(command: &mut Command) {
    use_program(command, Path::new(REFUSING_PROGRAM));
}

type OnPrompt = Box<dyn Fn(u64, PromptKind) -> bool + Send + Sync>;
type OnDismissed = Box<dyn Fn(u64) + Send + Sync>;

struct Shared {
    token: String,
    closed: AtomicBool,
    next: AtomicU64,
    pending: Mutex<HashMap<u64, Sender<Option<String>>>>,
    on_prompt: OnPrompt,
    on_dismissed: OnDismissed,
}

pub struct Server {
    shared: Arc<Shared>,
    client: ClientEnv,
    // Dropping it deletes the socket.
    _folder: TempDir,
}

impl Server {
    // `on_prompt` returns whether the question reached anyone: if not, nobody would ever answer and
    // it is rejected. `on_dismissed` reports that whoever asked no longer waits (the task was cancelled
    // or died): that way the dialog does not stay open for nobody. The socket lives in a `0700`
    // directory, the owner's alone.
    pub fn start(
        on_prompt: impl Fn(u64, PromptKind) -> bool + Send + Sync + 'static,
        on_dismissed: impl Fn(u64) + Send + Sync + 'static,
    ) -> io::Result<Self> {
        let program = env::current_exe()?;
        let token = random_token()?;
        // The mode is given on creation: `tempfile` does not restrict it by itself.
        let folder = tempfile::Builder::new()
            .prefix(SOCKET_FOLDER_PREFIX)
            .rand_bytes(SOCKET_FOLDER_RANDOM_CHARS)
            .permissions(Permissions::from_mode(0o700))
            .tempdir_in(socket_folder(&env::temp_dir()))?;
        let socket = folder.path().join(SOCKET_NAME);
        let listener = UnixListener::bind(&socket)?;
        let shared = Arc::new(Shared {
            token: token.clone(),
            closed: AtomicBool::new(false),
            next: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            on_prompt: Box::new(on_prompt),
            on_dismissed: Box::new(on_dismissed),
        });
        let accepting = shared.clone();
        thread::spawn(move || accept(listener, accepting));
        Ok(Self {
            shared,
            client: ClientEnv {
                program,
                socket,
                token,
            },
            _folder: folder,
        })
    }

    pub fn client_env(&self) -> ClientEnv {
        self.client.clone()
    }

    // `None` cancels. If nobody waits anymore, nothing happens.
    pub fn answer(&self, id: u64, answer: Option<String>) {
        if let Some(waiting) = self.shared.remove(id) {
            let _ = waiting.send(answer);
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::SeqCst);
        // Wakes `accept`, which is blocked waiting for a connection.
        let _ = UnixStream::connect(&self.client.socket);
    }
}

impl Shared {
    fn remove(&self, id: u64) -> Option<Sender<Option<String>>> {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id)
    }
}

// A long `TMPDIR` does not fit in the socket's address. The own directory is still `0700`, and
// creating it fails if it already exists, so `/tmp` loses nothing.
pub(crate) fn socket_folder(temp: &Path) -> PathBuf {
    let longest = temp.as_os_str().len()
        + 1
        + SOCKET_FOLDER_PREFIX.len()
        + SOCKET_FOLDER_RANDOM_CHARS
        + 1
        + SOCKET_NAME.len();
    if longest <= SOCKET_PATH_LIMIT {
        temp.to_path_buf()
    } else {
        PathBuf::from(SHORT_TEMP_FOLDER)
    }
}

pub(crate) fn random_token() -> io::Result<String> {
    let mut bytes = [0; TOKEN_BYTES];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
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

pub(crate) fn same_token(received: &[u8], token: &[u8]) -> bool {
    // Without exiting at the first differing byte: the timing must not say how many it got right.
    received
        .iter()
        .zip(token)
        .fold(0, |difference, (a, b)| difference | (a ^ b))
        == 0
}

// `<length, 4 bytes><token><question>`. The length goes first to reject a huge message without
// reading it, and so that ssh's multi-line questions need no escaping.
fn read_request(stream: &mut UnixStream, token: &str) -> io::Result<Option<String>> {
    let mut length = [0; size_of::<u32>()];
    stream.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if !(token.len()..=MAX_MESSAGE_BYTES).contains(&length) {
        return Ok(None);
    }
    let mut message = vec![0; length];
    stream.read_exact(&mut message)?;
    let (received, prompt) = message.split_at(token.len());
    Ok(
        same_token(received, token.as_bytes())
            .then(|| String::from_utf8_lossy(prompt).into_owned()),
    )
}

enum Outcome {
    Answered(String),
    Refused,
    ClientGone,
}

// While waiting, the client sends nothing else: reading can only yield the close, and incidentally
// it sets the pace of the wait.
fn wait_answer(stream: &mut UnixStream, answers: &Receiver<Option<String>>) -> Outcome {
    loop {
        match answers.try_recv() {
            Ok(Some(answer)) => return Outcome::Answered(answer),
            Ok(None) | Err(TryRecvError::Disconnected) => return Outcome::Refused,
            Err(TryRecvError::Empty) => {}
        }
        match stream.read(&mut [0]) {
            Ok(0) => return Outcome::ClientGone,
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(_) => return Outcome::ClientGone,
        }
    }
}

// Closing without writing anything is the rejection: a bad token gets no hint.
fn serve(mut stream: UnixStream, shared: &Shared) -> io::Result<()> {
    stream.set_read_timeout(Some(REQUEST_TIMEOUT))?;
    let Some(prompt) = read_request(&mut stream, &shared.token)? else {
        return Ok(());
    };
    // Before asking: on macOS, with the client already closed, this fails with `EINVAL`.
    stream.set_read_timeout(Some(POLL))?;
    let id = shared.next.fetch_add(1, Ordering::SeqCst);
    let (send, answers) = mpsc::channel();
    shared
        .pending
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(id, send);
    if !(shared.on_prompt)(id, classify(&prompt)) {
        shared.remove(id);
        return Ok(());
    }
    let outcome = wait_answer(&mut stream, &answers);
    shared.remove(id);
    match outcome {
        Outcome::Answered(answer) => {
            let mut reply = answer.into_bytes();
            reply.push(b'\n');
            stream.write_all(&reply)
        }
        Outcome::Refused => Ok(()),
        Outcome::ClientGone => {
            (shared.on_dismissed)(id);
            Ok(())
        }
    }
}

fn write_request(stream: &mut UnixStream, token: &str, prompt: &str) -> io::Result<()> {
    let length = u32::try_from(token.len() + prompt.len()).map_err(io::Error::other)?;
    let mut message = length.to_be_bytes().to_vec();
    message.extend_from_slice(token.as_bytes());
    message.extend_from_slice(prompt.as_bytes());
    stream.write_all(&message)
}

fn ask(socket: &Path, token: &str, prompt: &OsStr) -> io::Result<Vec<u8>> {
    let mut stream = UnixStream::connect(socket)?;
    write_request(&mut stream, token, &prompt.to_string_lossy())?;
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer)?;
    Ok(answer)
}

// Git and ssh launch the app's binary with the question as an argument. These two variables are only
// set by `ClientEnv`: without them it is the normal app. Exiting with 1 makes git give up without waiting.
pub fn client_main() -> Option<i32> {
    let socket = env::var_os(SOCKET_VARIABLE)?;
    let token = env::var(TOKEN_VARIABLE).ok()?;
    let prompt = env::args_os().nth(1).unwrap_or_default();
    let answer = ask(Path::new(&socket), &token, &prompt).unwrap_or_default();
    let answered = !answer.is_empty() && io::stdout().write_all(&answer).is_ok();
    Some(if answered { 0 } else { 1 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn username(url: &str) -> PromptKind {
        PromptKind::Username {
            url: url.to_string(),
        }
    }

    fn password(url: &str, user: &str) -> PromptKind {
        PromptKind::Password {
            url: url.to_string(),
            user: user.to_string(),
        }
    }

    #[test]
    fn classify_recognizes_https_username() {
        assert_eq!(
            classify("Username for 'https://github.com': "),
            username("https://github.com")
        );
    }

    #[test]
    fn classify_recognizes_https_password() {
        assert_eq!(
            classify("Password for 'https://me@github.com': "),
            password("https://github.com", "me")
        );
        // The user may contain an at-sign; the path too.
        assert_eq!(
            classify("Password for 'https://me@company.com@host:8443/a@b.git': "),
            password("https://host:8443/a@b.git", "me@company.com")
        );
    }

    #[test]
    fn classify_recognizes_passphrase() {
        for prompt in [
            "Enter passphrase for key '/Users/me/.ssh/id_ed25519': ",
            "Enter passphrase for /Users/me/.ssh/id_ed25519: ",
            "\rEnter passphrase for key '/Users/me/.ssh/id_ed25519':",
        ] {
            assert_eq!(
                classify(prompt),
                PromptKind::Passphrase {
                    key: "/Users/me/.ssh/id_ed25519".to_string()
                },
                "{prompt}"
            );
        }
    }

    #[test]
    fn classify_recognizes_ssh_password() {
        assert_eq!(
            classify("git@example.com's password: "),
            PromptKind::SshPassword {
                user: "git@example.com".to_string()
            }
        );
    }

    #[test]
    fn classify_recognizes_unknown_host() {
        let text = "The authenticity of host 'example.com (10.0.0.1)' can't be established.\n\
                    ED25519 key fingerprint is SHA256:abc.\n\
                    This key is not known by any other names.\n\
                    Are you sure you want to continue connecting (yes/no/[fingerprint])? ";

        assert_eq!(
            classify(text),
            PromptKind::HostKey {
                text: text.trim_end().to_string()
            }
        );
    }

    #[test]
    fn classify_odd_text_asks_for_password() {
        assert_eq!(
            classify("Verification code: "),
            password("Verification code:", "")
        );
    }

    fn silent_server() -> Server {
        Server::start(|_, _| true, |_| {}).unwrap()
    }

    #[test]
    fn the_socket_folder_respects_sun_path() {
        // The longest name that can come out, separator included.
        let free = SOCKET_PATH_LIMIT - "/ramify-askpass-XXXXXX/s".len();
        let fits = format!("/{}", "a".repeat(free - 1));
        let long = format!("{fits}a");

        assert_eq!(socket_folder(Path::new(&fits)), Path::new(&fits));
        assert_eq!(socket_folder(Path::new(&long)), Path::new("/tmp"));
    }

    // The limit is not an assumption: it is the one the system applies when opening the socket.
    #[test]
    fn the_path_limit_is_the_one_of_the_socket() {
        let folder = tempfile::Builder::new()
            .prefix("r")
            .tempdir_in("/tmp")
            .unwrap();
        let path = |length: usize| {
            let used = folder.path().as_os_str().len() + 1;
            folder.path().join("a".repeat(length - used))
        };

        assert!(UnixListener::bind(path(SOCKET_PATH_LIMIT)).is_ok());
        assert!(UnixListener::bind(path(SOCKET_PATH_LIMIT + 1)).is_err());
    }

    #[test]
    fn client_that_dies_while_asking_withdraws_the_question() {
        let client = Arc::new(Mutex::new(None::<UnixStream>));
        let held = client.clone();
        let (gone, dismissed) = mpsc::channel();
        let server = Server::start(
            // On macOS, changing options of a socket whose end already closed gives `EINVAL`.
            move |_, _| {
                held.lock().unwrap().take();
                true
            },
            move |id| gone.send(id).unwrap(),
        )
        .unwrap();
        let env = server.client_env();
        let mut stream = UnixStream::connect(&env.socket).unwrap();
        *client.lock().unwrap() = Some(stream.try_clone().unwrap());

        write_request(&mut stream, &env.token, "Password: ").unwrap();
        drop(stream);

        assert_eq!(dismissed.recv_timeout(Duration::from_secs(5)), Ok(1));
    }

    #[test]
    fn question_that_cannot_be_shown_is_rejected() {
        let (gone, dismissed) = mpsc::channel();
        let server = Server::start(|_, _| false, move |id| gone.send(id).unwrap()).unwrap();
        let env = server.client_env();
        let mut stream = UnixStream::connect(&env.socket).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();

        write_request(&mut stream, &env.token, "Password: ").unwrap();

        let mut answer = Vec::new();
        stream
            .read_to_end(&mut answer)
            .expect("the client keeps waiting");
        assert!(answer.is_empty());
        assert!(dismissed.try_recv().is_err());
    }

    #[test]
    fn socket_only_for_the_owner() {
        let server = silent_server();
        let client = server.client_env();

        let folder = client.socket.parent().unwrap();
        let mode = fs::metadata(folder).unwrap().permissions().mode();

        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn the_token_has_32_bytes_and_changes_on_every_run() {
        let first = silent_server().client_env().token;
        let second = silent_server().client_env().token;

        assert_eq!(first.len(), 64);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()), "format");
        assert_ne!(first, second);
    }

    #[test]
    fn message_over_64_kb_is_rejected() {
        let server = silent_server();
        let mut stream = UnixStream::connect(server.client_env().socket).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();

        let length = (MAX_MESSAGE_BYTES as u32 + 1).to_be_bytes();
        stream.write_all(&length).unwrap();

        // The server does not wait for the body: it cuts as soon as it sees the length.
        let closed = match stream.read(&mut [0]) {
            Ok(read) => read == 0,
            Err(error) => error.kind() != io::ErrorKind::WouldBlock,
        };
        assert!(closed, "the server keeps waiting for the body");
    }

    #[test]
    fn dropping_the_server_deletes_the_socket() {
        let server = silent_server();
        let folder = server.client_env().socket.parent().unwrap().to_path_buf();
        assert!(folder.is_dir());

        drop(server);

        assert!(!folder.exists());
    }
}
