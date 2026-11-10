// The askpass client is the app's binary: these tests launch the real one, the way git would,
// against a real server.
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use ramify_lib::askpass::{ClientEnv, PromptKind, Server};
use ramify_lib::commands::RepoLocks;
use ramify_lib::console::Console;
use ramify_lib::task::{GitOutput, Runner, TaskEvent, TaskId, TaskKind, TaskState};
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(20);
const SECRET: &str = "secret-that-must-not-leak";
const CLIENT: &str = env!("CARGO_BIN_EXE_ramify");

struct Fixture {
    server: Arc<Server>,
    prompts: Receiver<(u64, PromptKind)>,
    dismissed: Receiver<u64>,
}

impl Fixture {
    fn new() -> Self {
        let (asked, prompts) = mpsc::channel();
        let (gone, dismissed) = mpsc::channel();
        let server = Server::start(
            move |id, kind| asked.send((id, kind)).is_ok(),
            move |id| gone.send(id).unwrap(),
        )
        .unwrap();
        Self {
            server: Arc::new(server),
            prompts,
            dismissed,
        }
    }

    fn client(&self) -> ClientEnv {
        ClientEnv {
            program: PathBuf::from(CLIENT),
            ..self.server.client_env()
        }
    }

    fn next_prompt(&self) -> (u64, PromptKind) {
        self.prompts
            .recv_timeout(WAIT)
            .expect("no question arrived")
    }

    fn answer_next(&self, answer: Option<&str>) -> PromptKind {
        let (id, kind) = self.next_prompt();
        self.server.answer(id, answer.map(str::to_string));
        kind
    }
}

fn spawn_client(client: &ClientEnv, prompt: &str) -> thread::JoinHandle<Output> {
    let mut command = Command::new(&client.program);
    client.apply(&mut command);
    command.arg(prompt);
    thread::spawn(move || command.output().unwrap())
}

#[test]
fn client_returns_the_answer() {
    let fixture = Fixture::new();
    let client = spawn_client(&fixture.client(), "Password for 'https://me@host': ");

    let kind = fixture.answer_next(Some(SECRET));

    let output = client.join().unwrap();
    assert_eq!(
        kind,
        PromptKind::Password {
            url: "https://host".to_string(),
            user: "me".to_string()
        }
    );
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("{SECRET}\n")
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn client_without_a_valid_token_gets_rejected() {
    let fixture = Fixture::new();
    let client = ClientEnv {
        token: "0".repeat(64),
        ..fixture.client()
    };

    let client = spawn_client(&client, "Password: ");

    // If the server accepted it, the client would be left waiting for an answer: this goes first.
    assert!(fixture
        .prompts
        .recv_timeout(Duration::from_secs(2))
        .is_err());
    let output = client.join().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
}

#[test]
fn client_without_an_answer_exits_with_1() {
    let fixture = Fixture::new();
    let client = spawn_client(&fixture.client(), "Password: ");

    fixture.answer_next(None);

    let output = client.join().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
}

#[test]
fn an_empty_answer_is_not_a_rejection() {
    let fixture = Fixture::new();
    let client = spawn_client(&fixture.client(), "Password: ");

    fixture.answer_next(Some(""));

    let output = client.join().unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"\n");
}

#[test]
fn if_the_client_dies_the_question_is_withdrawn() {
    let fixture = Fixture::new();
    let client = fixture.client();
    let mut command = Command::new(&client.program);
    client.apply(&mut command);
    let mut child = command
        .arg("Password: ")
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let (id, _) = fixture.next_prompt();

    child.kill().unwrap();
    child.wait().unwrap();

    assert_eq!(fixture.dismissed.recv_timeout(WAIT), Ok(id));
    // An answer that arrives late bothers nobody.
    fixture.server.answer(id, Some(SECRET.to_string()));
}

// An ssh that asks like the real one and, if it gets an answer, serves the local repository.
// `git` launches it in the repository: what it records stays there, one copy per test.
const FAKE_SSH: &str = r#"#!/bin/sh
[ "$1" = "-G" ] && exit 1
echo $$ > pid
printf 'GIT_TERMINAL_PROMPT=%s\nSSH_ASKPASS_REQUIRE=%s\n' \
  "$GIT_TERMINAL_PROMPT" "$SSH_ASKPASS_REQUIRE" > env
"$SSH_ASKPASS" "git@host's password: " > answer
echo $? > exit_code
[ -s answer ] || exit 255
for last; do :; done
exec sh -c "git ${last#git-}"
"#;

// The environment belongs to the whole process, so there is a single fake ssh for all the tests.
fn install_fake_ssh() {
    static FOLDER: OnceLock<TempDir> = OnceLock::new();
    FOLDER.get_or_init(|| {
        let folder = TempDir::new().unwrap();
        let script = folder.path().join("fake-ssh");
        fs::write(&script, FAKE_SSH).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        env::set_var("GIT_SSH_COMMAND", &script);
        folder
    });
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(repo)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

struct Remote {
    origin: TempDir,
    clone: TempDir,
}

impl Remote {
    fn new() -> Self {
        let origin = TempDir::new().unwrap();
        git(origin.path(), &["init", "-q"]);
        git(
            origin.path(),
            &[
                "-c",
                "user.name=Ramify",
                "-c",
                "user.email=Ramify@example.com",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "one",
            ],
        );
        let clone = TempDir::new().unwrap();
        git(clone.path(), &["init", "-q"]);
        Self { origin, clone }
    }

    fn url(&self) -> String {
        format!("ssh://git@host{}", self.origin.path().display())
    }

    fn mark(&self, name: &str) -> String {
        fs::read_to_string(self.clone.path().join(name)).unwrap_or_default()
    }
}

struct Tasks {
    runner: Runner,
    console: Arc<Console>,
    events: Receiver<TaskEvent>,
}

impl Tasks {
    fn new(client: ClientEnv) -> Self {
        let tasks = Self::without_askpass();
        tasks.runner.use_askpass(client);
        tasks
    }

    fn without_askpass() -> Self {
        let console = Arc::new(Console::default());
        let runner = Runner::new(Arc::new(RepoLocks::default()), console.clone());
        let (send, events) = mpsc::channel();
        let send = std::sync::Mutex::new(send);
        runner.listen(move |event| {
            let _ = send.lock().unwrap().send(event.clone());
        });
        Self {
            runner,
            console,
            events,
        }
    }

    fn git_loud(
        &self,
        repo: &Path,
        args: &[&str],
    ) -> (TaskId, Receiver<Result<GitOutput, String>>) {
        let (send, result) = mpsc::channel();
        let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        let id = self
            .runner
            .start(repo, TaskKind::Read, "test".to_string(), move |context| {
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                let output = context.git_loud(&args);
                let _ = send.send(output.clone().map_err(|error| format!("{error:?}")));
                output.map(|_| ())
            });
        (id, result)
    }

    fn until_finished(&self) -> Vec<TaskEvent> {
        let mut seen = Vec::new();
        loop {
            let event = self.events.recv_timeout(WAIT).expect("the task did not finish");
            let done = event.state != TaskState::Running;
            seen.push(event);
            if done {
                return seen;
            }
        }
    }
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 reaches nobody: it only asks whether the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

fn wait_until_dead(pid: i32) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(pid) {
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
    true
}

#[test]
fn git_with_askpass_without_a_terminal() {
    install_fake_ssh();
    let fixture = Fixture::new();
    let tasks = Tasks::new(fixture.client());
    let remote = Remote::new();

    let (_, result) = tasks.git_loud(remote.clone.path(), &["ls-remote", &remote.url()]);
    let kind = fixture.answer_next(Some(SECRET));

    let output = result.recv_timeout(WAIT).unwrap().unwrap();
    assert_eq!(
        kind,
        PromptKind::SshPassword {
            user: "git@host".to_string()
        }
    );
    assert!(output.stdout.contains("HEAD"), "{}", output.stdout);
    assert_eq!(remote.mark("answer"), format!("{SECRET}\n"));
    assert_eq!(
        remote.mark("env"),
        "GIT_TERMINAL_PROMPT=0\nSSH_ASKPASS_REQUIRE=force\n"
    );
}

#[test]
fn without_an_answer_git_is_cancelled() {
    install_fake_ssh();
    let fixture = Fixture::new();
    let tasks = Tasks::new(fixture.client());
    let remote = Remote::new();

    let (_, result) = tasks.git_loud(remote.clone.path(), &["ls-remote", &remote.url()]);
    fixture.answer_next(None);

    assert!(result.recv_timeout(WAIT).unwrap().is_err());
    assert_eq!(remote.mark("exit_code"), "1\n");
}

#[test]
fn cancelling_the_task_withdraws_the_question() {
    install_fake_ssh();
    let fixture = Fixture::new();
    let tasks = Tasks::new(fixture.client());
    let remote = Remote::new();
    let (task, result) = tasks.git_loud(remote.clone.path(), &["ls-remote", &remote.url()]);
    let (id, _) = fixture.next_prompt();
    let ssh: i32 = remote.mark("pid").trim().parse().unwrap();
    assert!(alive(ssh));

    tasks.runner.cancel(task);

    assert!(wait_until_dead(ssh), "ssh is still alive");
    assert_eq!(fixture.dismissed.recv_timeout(WAIT), Ok(id));
    assert_eq!(
        result.recv_timeout(WAIT).unwrap().map(|_| ()),
        Err("Cancelled".to_string())
    );
}

// Without a server there is nobody to ask: ssh must fail, not go to `/dev/tty`.
#[test]
fn without_a_server_ssh_fails_without_a_terminal() {
    install_fake_ssh();
    let tasks = Tasks::without_askpass();
    let remote = Remote::new();

    let (_, result) = tasks.git_loud(remote.clone.path(), &["ls-remote", &remote.url()]);

    assert!(result.recv_timeout(WAIT).unwrap().is_err());
    assert_eq!(remote.mark("exit_code"), "1\n");
    assert_eq!(
        remote.mark("env"),
        "GIT_TERMINAL_PROMPT=0\nSSH_ASKPASS_REQUIRE=force\n"
    );
}

#[test]
fn the_answer_does_not_reach_the_console() {
    install_fake_ssh();
    let fixture = Fixture::new();
    let tasks = Tasks::new(fixture.client());
    let remote = Remote::new();

    let (_, result) = tasks.git_loud(remote.clone.path(), &["ls-remote", &remote.url()]);
    fixture.answer_next(Some(SECRET));
    result.recv_timeout(WAIT).unwrap().unwrap();
    let events = tasks.until_finished();

    let lines = tasks.console.lines();
    assert!(lines.iter().any(|line| line.command), "{lines:?}");
    assert!(
        lines.iter().all(|line| !line.text.contains(SECRET)),
        "{lines:?}"
    );
    assert!(
        events
            .iter()
            .all(|event| !format!("{event:?}").contains(SECRET)),
        "{events:?}"
    );
}

// A minimal HTTP server that always asks for credentials and counts the ones it receives.
fn http_requiring_auth() -> (String, Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (send, received) = mpsc::channel();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let reader = BufReader::new(stream.try_clone().unwrap());
            for line in reader.lines() {
                let line = line.unwrap();
                if line.is_empty() {
                    break;
                }
                if let Some(value) = line.strip_prefix("Authorization: Basic ") {
                    let _ = send.send(value.to_string());
                }
            }
            let _ = stream.write_all(
                b"HTTP/1.1 401 Unauthorized\r\n\
                  WWW-Authenticate: Basic realm=\"Ramify\"\r\n\
                  Content-Length: 0\r\n\
                  Connection: close\r\n\r\n",
            );
        }
    });
    (url, received)
}

// Without a helper: this test must not touch the system keychain.
#[test]
fn git_asks_for_username_and_password_over_https() {
    let fixture = Fixture::new();
    let tasks = Tasks::new(fixture.client());
    tasks.runner.set_credential_helper(false);
    let (url, received) = http_requiring_auth();
    let repo = TempDir::new().unwrap();
    git(repo.path(), &["init", "-q"]);

    let (_, result) = tasks.git_loud(repo.path(), &["ls-remote", &format!("{url}/r.git")]);
    let asked_user = fixture.answer_next(Some("me"));
    let asked_password = fixture.answer_next(Some("pass"));

    assert_eq!(asked_user, PromptKind::Username { url: url.clone() });
    assert_eq!(
        asked_password,
        PromptKind::Password {
            url,
            user: "me".to_string()
        }
    );
    // `me:pass` in base64.
    assert_eq!(received.recv_timeout(WAIT).unwrap(), "bWU6cGFzcw==");
    let error = result.recv_timeout(WAIT).unwrap().unwrap_err();
    assert!(!error.contains("pass"), "{error}");
}
