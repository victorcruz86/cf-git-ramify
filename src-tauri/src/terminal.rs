// The Terminal tab: interactive shells in pseudo-terminals, one per session, started in the
// repository of the project window that opens them. Every other process the app starts runs git
// with arguments and no shell; this one is a shell on purpose, because what runs there is what the
// user types. Nothing the interface sends reaches the command line: the program is the user's login
// shell and the folder is the window's repository, both decided here.

use std::collections::HashMap;
use std::env;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, State, WebviewWindow};

use crate::commands::{failed, owner_label, repo_of, ApiError, AppState};

// What the shell is told it runs in. xterm.js answers as `xterm-256color` and draws true colour.
const TERM: &str = "xterm-256color";
// An app started from the Finder has no `LANG`; without one, zsh shows UTF-8 names as escapes.
const FALLBACK_LANG: &str = "en_US.UTF-8";
// How long a hung-up session has to end before it is killed.
const HANG_UP_GRACE: Duration = Duration::from_secs(2);
// The size of each read of the shell's output.
const READ_CHUNK: usize = 16 * 1024;

// What a session runs, and where.
pub struct Launch<'a> {
    pub program: &'a Path,
    pub args: &'a [&'a str],
    pub cwd: &'a Path,
}

#[derive(Clone, Copy, Debug)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
}

struct Session {
    // The label of the project window's main window: only its pages may use the session.
    owner: String,
    pid: i32,
    master: File,
    // False once the shell has been waited for: its pid may belong to someone else from then on.
    alive: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct Terminals {
    next: AtomicU64,
    sessions: Arc<Mutex<HashMap<u64, Arc<Session>>>>,
}

// The user's login shell, as Terminal.app starts it: `$SHELL` when it names a program, zsh otherwise.
pub fn login_shell() -> PathBuf {
    env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.is_file())
        .unwrap_or_else(|| PathBuf::from("/bin/zsh"))
}

fn winsize(size: Size) -> libc::winsize {
    libc::winsize {
        ws_row: size.rows.max(1),
        ws_col: size.cols.max(1),
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

fn os_result(value: libc::c_int) -> io::Result<libc::c_int> {
    if value == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(value)
    }
}

impl Terminals {
    // Starts what `launch` says on a new pseudo-terminal of `size`. Its output goes to `output`
    // as it comes; `exited` runs once the program has ended and been waited for.
    pub fn open(
        &self,
        owner: &str,
        launch: Launch,
        size: Size,
        mut output: impl FnMut(Vec<u8>) + Send + 'static,
        exited: impl FnOnce(u64) + Send + 'static,
    ) -> io::Result<u64> {
        let mut master: RawFd = -1;
        let mut slave: RawFd = -1;
        let mut window = winsize(size);
        // SAFETY: the pointers are to live locals; openpty fills the two descriptors.
        os_result(unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut window,
            )
        })?;
        // SAFETY: openpty succeeded, so both are open descriptors that nobody else owns.
        let master = unsafe { File::from_raw_fd(master) };
        let slave = unsafe { File::from_raw_fd(slave) };
        // The shell must not inherit the master side, or it would never see the hang-up.
        // SAFETY: a valid descriptor and a flag.
        os_result(unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) })?;

        let mut command = Command::new(launch.program);
        command
            .args(launch.args)
            .current_dir(launch.cwd)
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave))
            .env("TERM", TERM)
            .env("COLORTERM", "truecolor")
            .env("TERM_PROGRAM", "Ramify");
        if env::var_os("LANG").is_none() {
            command.env("LANG", FALLBACK_LANG);
        }
        // SAFETY: only async-signal-safe calls between fork and exec. The shell leads a new
        // session whose controlling terminal is the pseudo-terminal, so job control works.
        unsafe {
            command.pre_exec(|| {
                os_result(libc::setsid())?;
                os_result(libc::ioctl(0, libc::TIOCSCTTY as _, 0))?;
                Ok(())
            });
        }
        let mut child = command.spawn()?;
        drop(command);

        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let alive = Arc::new(AtomicBool::new(true));
        let mut reader = master.try_clone()?;
        let session = Arc::new(Session {
            owner: owner.to_string(),
            pid: child.id() as i32,
            master,
            alive: alive.clone(),
        });
        self.sessions.lock().unwrap().insert(id, session);

        thread::spawn(move || {
            let mut buffer = vec![0; READ_CHUNK];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => output(buffer[..read].to_vec()),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    // EIO: nobody holds the terminal's other side any more.
                    Err(_) => break,
                }
            }
        });
        let sessions = self.sessions.clone();
        thread::spawn(move || {
            let _ = child.wait();
            alive.store(false, Ordering::SeqCst);
            sessions.lock().unwrap().remove(&id);
            exited(id);
        });
        Ok(id)
    }

    fn owned(&self, id: u64, owner: &str) -> io::Result<Arc<Session>> {
        self.sessions
            .lock()
            .unwrap()
            .get(&id)
            .filter(|session| session.owner == owner)
            .cloned()
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "The terminal session has ended")
            })
    }

    pub fn write(&self, id: u64, owner: &str, data: &[u8]) -> io::Result<()> {
        let session = self.owned(id, owner)?;
        (&session.master).write_all(data)
    }

    pub fn resize(&self, id: u64, owner: &str, size: Size) -> io::Result<()> {
        let session = self.owned(id, owner)?;
        let window = winsize(size);
        // SAFETY: a valid descriptor and a pointer to a live winsize.
        os_result(unsafe { libc::ioctl(session.master.as_raw_fd(), libc::TIOCSWINSZ, &window) })?;
        Ok(())
    }

    // Whether a program other than the shell runs in the foreground: closing would end it.
    pub fn busy(&self, id: u64, owner: &str) -> bool {
        let Ok(session) = self.owned(id, owner) else {
            return false;
        };
        // SAFETY: a valid descriptor.
        let foreground = unsafe { libc::tcgetpgrp(session.master.as_raw_fd()) };
        foreground > 0 && foreground != session.pid
    }

    pub fn close(&self, id: u64, owner: &str) {
        let session = {
            let mut sessions = self.sessions.lock().unwrap();
            match sessions.get(&id) {
                Some(session) if session.owner == owner => sessions.remove(&id),
                _ => None,
            }
        };
        if let Some(session) = session {
            hang_up(&session);
        }
    }

    // Every session of a project window, when it closes or its page starts again.
    pub fn close_owner(&self, owner: &str) {
        let ended: Vec<Arc<Session>> = {
            let mut sessions = self.sessions.lock().unwrap();
            let ids: Vec<u64> = sessions
                .iter()
                .filter(|(_, session)| session.owner == owner)
                .map(|(id, _)| *id)
                .collect();
            ids.iter().filter_map(|id| sessions.remove(id)).collect()
        };
        ended.iter().for_each(|session| hang_up(session));
    }

    pub fn close_all(&self) {
        let ended: Vec<Arc<Session>> = self
            .sessions
            .lock()
            .unwrap()
            .drain()
            .map(|(_, s)| s)
            .collect();
        ended.iter().for_each(|session| hang_up(session));
    }
}

// What a terminal window's close does: SIGHUP to the shell and to the program in the foreground,
// and SIGKILL to both if the shell is still there after a grace period.
fn hang_up(session: &Session) {
    // SAFETY: a valid descriptor.
    let foreground = unsafe { libc::tcgetpgrp(session.master.as_raw_fd()) };
    let groups: Vec<i32> = std::iter::once(session.pid)
        .chain((foreground > 0 && foreground != session.pid).then_some(foreground))
        .collect();
    for group in &groups {
        // SAFETY: a signal to a process group this session started.
        unsafe { libc::kill(-group, libc::SIGHUP) };
    }
    let alive = session.alive.clone();
    thread::spawn(move || {
        thread::sleep(HANG_UP_GRACE);
        if alive.load(Ordering::SeqCst) {
            for group in &groups {
                // SAFETY: as above; `alive` says the shell has not been waited for yet.
                unsafe { libc::kill(-group, libc::SIGKILL) };
            }
        }
    });
}

fn size_of(cols: u16, rows: u16) -> Size {
    Size { cols, rows }
}

fn io_failed(error: io::Error) -> ApiError {
    failed(error.to_string())
}

#[tauri::command]
pub fn terminal_open(
    cols: u16,
    rows: u16,
    output: Channel<InvokeResponseBody>,
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<u64, ApiError> {
    let owner = owner_label(&window);
    let cwd = repo_of(&state, window.label())?;
    let label = window.label().to_string();
    state
        .terminals
        .open(
            &owner,
            Launch {
                program: &login_shell(),
                args: &["-l"],
                cwd: &cwd,
            },
            size_of(cols, rows),
            move |bytes| {
                let _ = output.send(InvokeResponseBody::Raw(bytes));
            },
            move |id| {
                let _ = app.emit_to(label.as_str(), "terminal-exited", id);
            },
        )
        .map_err(|error| failed(format!("Could not start the terminal: {error}")))
}

#[tauri::command]
pub fn terminal_write(
    id: u64,
    data: String,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    state
        .terminals
        .write(id, &owner_label(&window), data.as_bytes())
        .map_err(io_failed)
}

#[tauri::command]
pub fn terminal_resize(
    id: u64,
    cols: u16,
    rows: u16,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<(), ApiError> {
    state
        .terminals
        .resize(id, &owner_label(&window), size_of(cols, rows))
        .map_err(io_failed)
}

#[tauri::command]
pub fn terminal_busy(id: u64, window: WebviewWindow, state: State<'_, AppState>) -> bool {
    state.terminals.busy(id, &owner_label(&window))
}

#[tauri::command]
pub fn terminal_close(id: u64, window: WebviewWindow, state: State<'_, AppState>) {
    state.terminals.close(id, &owner_label(&window));
}

// A page that starts again has lost its sessions' views: their shells end with them.
#[tauri::command]
pub fn terminal_reset(window: WebviewWindow, state: State<'_, AppState>) {
    state.terminals.close_owner(&owner_label(&window));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Instant;

    const OWNER: &str = "main";
    const WAIT: Duration = Duration::from_secs(10);

    struct Opened {
        id: u64,
        output: mpsc::Receiver<Vec<u8>>,
        exited: mpsc::Receiver<u64>,
        seen: String,
        cwd: PathBuf,
    }

    impl Opened {
        // Reads output until it contains `text`, or fails after `WAIT`.
        fn expect(&mut self, text: &str) {
            let deadline = Instant::now() + WAIT;
            while !self.seen.contains(text) {
                let left = deadline.saturating_duration_since(Instant::now());
                match self.output.recv_timeout(left) {
                    Ok(bytes) => self.seen.push_str(&String::from_utf8_lossy(&bytes)),
                    Err(_) => panic!("{text:?} never came; the terminal showed {:?}", self.seen),
                }
            }
        }
    }

    fn open_sh(terminals: &Terminals, size: Size) -> Opened {
        let (output_tx, output) = mpsc::channel();
        let (exited_tx, exited) = mpsc::channel();
        let cwd = tempfile::tempdir().unwrap().keep();
        let id = terminals
            .open(
                OWNER,
                Launch {
                    program: Path::new("/bin/sh"),
                    args: &[],
                    cwd: &cwd,
                },
                size,
                move |bytes| {
                    let _ = output_tx.send(bytes);
                },
                move |id| {
                    let _ = exited_tx.send(id);
                },
            )
            .unwrap();
        Opened {
            id,
            output,
            exited,
            seen: String::new(),
            cwd,
        }
    }

    #[test]
    fn a_session_runs_what_is_typed_and_shows_its_output() {
        let terminals = Terminals::default();
        let mut shell = open_sh(&terminals, size_of(80, 24));
        terminals
            .write(shell.id, OWNER, b"echo ramify-$((40+2))\n")
            .unwrap();
        shell.expect("ramify-42");
    }

    #[test]
    fn the_shell_starts_in_the_folder_it_was_given() {
        let terminals = Terminals::default();
        let mut shell = open_sh(&terminals, size_of(80, 24));
        let name = shell
            .cwd
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        terminals
            .write(shell.id, OWNER, b"echo in-$(basename \"$(pwd -P)\")-end\n")
            .unwrap();
        shell.expect(&format!("in-{name}-end"));
    }

    #[test]
    fn the_terminal_has_the_size_it_was_given_and_follows_a_resize() {
        let terminals = Terminals::default();
        let mut shell = open_sh(&terminals, size_of(100, 30));
        terminals.write(shell.id, OWNER, b"stty size\n").unwrap();
        shell.expect("30 100");
        terminals.resize(shell.id, OWNER, size_of(120, 40)).unwrap();
        terminals.write(shell.id, OWNER, b"stty size\n").unwrap();
        shell.expect("40 120");
    }

    #[test]
    fn the_shell_sees_a_colour_terminal() {
        let terminals = Terminals::default();
        let mut shell = open_sh(&terminals, size_of(80, 24));
        terminals
            .write(shell.id, OWNER, b"echo term=$TERM\n")
            .unwrap();
        shell.expect("term=xterm-256color");
    }

    #[test]
    fn exit_ends_the_session_and_says_so() {
        let terminals = Terminals::default();
        let shell = open_sh(&terminals, size_of(80, 24));
        terminals.write(shell.id, OWNER, b"exit\n").unwrap();
        assert_eq!(shell.exited.recv_timeout(WAIT).unwrap(), shell.id);
        assert!(terminals.write(shell.id, OWNER, b"echo late\n").is_err());
    }

    #[test]
    fn a_program_in_the_foreground_makes_the_session_busy() {
        let terminals = Terminals::default();
        let mut shell = open_sh(&terminals, size_of(80, 24));
        terminals.write(shell.id, OWNER, b"echo ready\n").unwrap();
        shell.expect("ready");
        assert!(!terminals.busy(shell.id, OWNER));
        terminals.write(shell.id, OWNER, b"sleep 30\n").unwrap();
        let deadline = Instant::now() + WAIT;
        while !terminals.busy(shell.id, OWNER) {
            assert!(Instant::now() < deadline, "sleep never took the foreground");
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn closing_ends_the_shell_and_the_program_it_runs() {
        let terminals = Terminals::default();
        let mut shell = open_sh(&terminals, size_of(80, 24));
        terminals.write(shell.id, OWNER, b"echo ready\n").unwrap();
        shell.expect("ready");
        terminals.write(shell.id, OWNER, b"sleep 30\n").unwrap();
        thread::sleep(Duration::from_millis(200));
        terminals.close(shell.id, OWNER);
        assert_eq!(shell.exited.recv_timeout(WAIT).unwrap(), shell.id);
    }

    #[test]
    fn another_window_cannot_use_or_close_a_session() {
        let terminals = Terminals::default();
        let mut shell = open_sh(&terminals, size_of(80, 24));
        assert!(terminals
            .write(shell.id, "main-2", b"echo intruder\n")
            .is_err());
        assert!(terminals
            .resize(shell.id, "main-2", size_of(10, 10))
            .is_err());
        terminals.close(shell.id, "main-2");
        terminals
            .write(shell.id, OWNER, b"echo still-here\n")
            .unwrap();
        shell.expect("still-here");
        assert!(!shell.seen.contains("intruder"));
    }

    #[test]
    fn closing_a_window_ends_only_its_sessions() {
        let terminals = Terminals::default();
        let mine = open_sh(&terminals, size_of(80, 24));
        let (output_tx, output) = mpsc::channel();
        let other = terminals
            .open(
                "main-2",
                Launch {
                    program: Path::new("/bin/sh"),
                    args: &[],
                    cwd: &tempfile::tempdir().unwrap().keep(),
                },
                size_of(80, 24),
                move |bytes| {
                    let _ = output_tx.send(bytes);
                },
                |_| {},
            )
            .unwrap();
        terminals.close_owner(OWNER);
        assert_eq!(mine.exited.recv_timeout(WAIT).unwrap(), mine.id);
        let mut theirs = Opened {
            id: other,
            output,
            exited: mpsc::channel().1,
            seen: String::new(),
            cwd: PathBuf::new(),
        };
        terminals.write(other, "main-2", b"echo alive\n").unwrap();
        theirs.expect("alive");
        terminals.close_all();
    }
}
