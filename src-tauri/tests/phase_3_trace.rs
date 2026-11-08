// For `docs/reports/phase-3-commands.md`: records what the app launches in merge, Smart Merge,
// rebase, interactive rebase, cherry-pick, revert, undo commit, reword, drop, squash and
// conflict resolution. It is run with `scripts/git-wrapper.sh` as `git`; the log is only
// active while each operation runs, not during the setup.
//
// It goes through the core (the functions the Tauri commands use) inside a `Runner` task,
// with a real editor server. It does not go through the IPC or the interface.
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rama_lib::apply_commits::{self, Kind, Selection};
use rama_lib::branches::Repo;
use rama_lib::commands::RepoLocks;
use rama_lib::commit;
use rama_lib::conflicts::{self, Side};
use rama_lib::console::Console;
use rama_lib::editor::Server;
use rama_lib::merge;
use rama_lib::operation::{self, Operation};
use rama_lib::rebase::{self, RebaseParams};
use rama_lib::rewrite::{self, NewMessage, Plan};
use rama_lib::task::{Runner, TaskContext, TaskKind};
use rama_lib::todo::{Action, Entry};
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(60);
const CLIENT: &str = env!("CARGO_BIN_EXE_rama");

fn git_in(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_EDITOR", "true")
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).into_owned()
}

struct Scratch {
    dir: TempDir,
}

impl Scratch {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        git_in(dir.path(), &["init", "-q", "-b", "main"]);
        for (key, value) in [
            ("user.name", "Test"),
            ("user.email", "test@example.com"),
            ("commit.gpgsign", "false"),
        ] {
            git_in(dir.path(), &["config", key, value]);
        }
        Self { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn git(&self, args: &[&str]) -> String {
        git_in(self.path(), args)
    }

    fn write(&self, file: &str, contents: &str) {
        fs::write(self.path().join(file), contents).unwrap();
    }

    fn commit(&self, file: &str, contents: &str, message: &str) -> String {
        self.write(file, contents);
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
        self.head()
    }

    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    // `main` and `side` start from `a.txt = base` and both change it; it stays on `main`.
    fn diverged() -> Self {
        let repo = Self::new();
        repo.commit("a.txt", "base\n", "base");
        repo.git(&["checkout", "-q", "-b", "side"]);
        repo.commit("a.txt", "side\n", "clashing");
        repo.commit("b.txt", "b\n", "follows");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("a.txt", "main\n", "main");
        repo
    }

    // The same, without them clashing: `side` adds `b.txt` and `main` adds `m.txt`.
    fn apart() -> Self {
        let repo = Self::new();
        repo.commit("a.txt", "base\n", "base");
        repo.git(&["checkout", "-q", "-b", "side"]);
        repo.commit("b.txt", "b\n", "one");
        repo.commit("c.txt", "c\n", "two");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit("m.txt", "m\n", "main");
        repo
    }
}

struct Fixture {
    _server: Arc<Server>,
    runner: Runner,
}

impl Fixture {
    fn new() -> Self {
        // Whoever uses the app would accept the text as it is (Continue Rebase asks for the message).
        let (send, requests): (_, Receiver<(u64, String)>) = mpsc::channel();
        let send = Mutex::new(send);
        let server = Arc::new(
            Server::start_with_program(
                PathBuf::from(CLIENT),
                move |id, prompt| send.lock().unwrap().send((id, prompt.text)).is_ok(),
                |_| {},
            )
            .unwrap(),
        );
        let answering = server.clone();
        std::thread::spawn(move || {
            for (id, text) in requests {
                answering.answer(id, Some(text));
            }
        });
        let runner = Runner::new(Arc::new(RepoLocks::default()), Arc::new(Console::default()));
        runner.use_editor(server.handle());
        Self {
            _server: server,
            runner,
        }
    }

    fn run<T: Send + 'static>(
        &self,
        repo: &Scratch,
        work: impl FnOnce(&Repo) -> T + Send + 'static,
    ) -> T {
        let path = repo.path().to_path_buf();
        let (send, receive) = mpsc::channel();
        self.runner.start(
            repo.path(),
            TaskKind::Write,
            "trace".to_string(),
            move |ctx: &TaskContext| {
                let data = TempDir::new().unwrap();
                send.send(work(&Repo {
                    ctx,
                    path: &path,
                    data: data.path(),
                }))
                .unwrap();
                Ok(())
            },
        );
        receive.recv_timeout(WAIT).expect("the task did not finish")
    }
}

// Records only what runs inside `operation`.
struct Trace {
    log: std::ffi::OsString,
}

impl Trace {
    fn new() -> Self {
        let log = std::env::var_os("RAMA_GIT_LOG").expect("RAMA_GIT_LOG is missing");
        fs::write(&log, "").unwrap();
        std::env::remove_var("RAMA_GIT_LOG");
        Self { log }
    }

    fn section(&self, title: &str) {
        let mut file = fs::OpenOptions::new().append(true).open(&self.log).unwrap();
        writeln!(file, "## {title}").unwrap();
        std::env::set_var("RAMA_GIT_LOG", &self.log);
    }

    fn silence(&self) {
        std::env::remove_var("RAMA_GIT_LOG");
    }

    fn of<T: Send + 'static>(
        &self,
        title: &str,
        fixture: &Fixture,
        repo: &Scratch,
        work: impl FnOnce(&Repo) -> T + Send + 'static,
    ) -> T {
        self.section(title);
        let done = fixture.run(repo, work);
        self.silence();
        done
    }
}

// The entries of the interactive rebase dialog with the chosen actions.
fn planned(entries: Vec<Entry>, actions: &[(&str, Action)], messages: &[(&str, &str)]) -> Plan {
    let entries = entries
        .into_iter()
        .map(|mut entry| {
            if let Some((_, action)) = actions.iter().find(|(s, _)| *s == entry.subject) {
                entry.action = action.clone();
            }
            entry
        })
        .collect::<Vec<_>>();
    let messages = messages
        .iter()
        .map(|(subject, message)| NewMessage {
            hash: entries
                .iter()
                .find(|entry| entry.subject == *subject)
                .unwrap()
                .hash
                .clone(),
            message: message.to_string(),
        })
        .collect();
    Plan { entries, messages }
}

fn rebasing(side: &str) -> RebaseParams {
    RebaseParams {
        upstream: Some("main".to_string()),
        branch: Some(side.to_string()),
        ..RebaseParams::default()
    }
}

#[test]
#[ignore = "only to record commands with scripts/git-wrapper.sh"]
fn phase_3_trace() {
    let trace = Trace::new();
    let fixture = Fixture::new();
    let checked = |what: &str, ok: bool| assert!(ok, "{what}");

    // Merge without conflict, with a merge commit.
    let repo = Scratch::apart();
    let head = repo.head();
    let merged = trace.of("merge without conflict", &fixture, &repo, move |r| {
        merge::merge_into(r, "side", "local", false, &head)
    });
    checked("merge without conflict", merged.is_ok());

    // Merge with conflict; resolve with Apply and commit.
    let repo = Scratch::diverged();
    let head = repo.head();
    let merged = trace.of("merge with conflict", &fixture, &repo, move |r| {
        merge::merge_into(r, "side", "local", false, &head)
    });
    checked("merge with conflict", merged.is_ok());
    let sides = trace.of(
        "merge window: open the three versions",
        &fixture,
        &repo,
        |r| {
            let _ = conflicts::session(r);
            conflicts::sides(r, "a.txt")
        },
    );
    let hash = sides.unwrap().hash;
    let applied = trace.of(
        "Apply (result of the merge window)",
        &fixture,
        &repo,
        move |r| conflicts::resolve(r, "a.txt", b"resolved\n", &hash),
    );
    checked("apply", applied.is_ok());
    let confirmed = trace.of("commit the merge after resolving", &fixture, &repo, |r| {
        commit::merge_commit(r)
    });
    checked("merge_commit", confirmed.is_ok());

    // Accept Yours.
    let repo = Scratch::diverged();
    let head = repo.head();
    fixture
        .run(&repo, move |r| merge::merge_into(r, "side", "local", false, &head))
        .unwrap();
    let accepted = trace.of("Accept Yours in a merge", &fixture, &repo, |r| {
        conflicts::accept(r, &["a.txt".to_string()], Side::Yours)
    });
    checked("accept yours", accepted.is_ok());
    let confirmed = trace.of(
        "commit the merge after Accept Yours",
        &fixture,
        &repo,
        commit::merge_commit,
    );
    checked("merge_commit", confirmed.is_ok());

    // Accept Theirs.
    let repo = Scratch::diverged();
    let head = repo.head();
    fixture
        .run(&repo, move |r| merge::merge_into(r, "side", "local", false, &head))
        .unwrap();
    let accepted = trace.of("Accept Theirs in a merge", &fixture, &repo, |r| {
        conflicts::accept(r, &["a.txt".to_string()], Side::Theirs)
    });
    checked("accept theirs", accepted.is_ok());

    // Abort Merge.
    let repo = Scratch::diverged();
    let head = repo.head();
    fixture
        .run(&repo, move |r| merge::merge_into(r, "side", "local", false, &head))
        .unwrap();
    let aborted = trace.of("Abort Merge", &fixture, &repo, |r| {
        operation::abort(r, Operation::Merging)
    });
    checked("abort merge", aborted.is_ok());

    // Smart Merge: a local change that the merge would overwrite.
    let repo = Scratch::new();
    repo.commit("a.txt", "1\n2\n3\n", "base");
    repo.git(&["checkout", "-q", "-b", "side"]);
    repo.commit("a.txt", "1\n2\n3\n4\n", "side touches a");
    repo.git(&["checkout", "-q", "main"]);
    repo.commit("m.txt", "m\n", "main");
    repo.write("a.txt", "0\n1\n2\n3\n");
    let head = repo.head();
    let smart = trace.of(
        "Smart Merge with overlapping local changes",
        &fixture,
        &repo,
        move |r| merge::merge_into(r, "side", "local", true, &head),
    );
    checked("smart merge", smart.is_ok());

    // Rebase without conflict (Rebase… dialog).
    let repo = Scratch::apart();
    repo.git(&["checkout", "-q", "side"]);
    let head = repo.head();
    let params = RebaseParams {
        upstream: Some("main".to_string()),
        ..RebaseParams::default()
    };
    let rebased = trace.of(
        "rebase without conflict (Rebase…)",
        &fixture,
        &repo,
        move |r| rebase::rebase(r, &params, &head),
    );
    checked("rebase", rebased.is_ok());

    // Rebase with conflict: continue, skip and abort.
    let repo = Scratch::diverged();
    repo.git(&["checkout", "-q", "side"]);
    let head = repo.head();
    let rebased = trace.of("rebase with conflict", &fixture, &repo, {
        let params = rebasing("side");
        move |r| rebase::rebase(r, &params, &head)
    });
    checked("rebase with conflict", rebased.is_ok());
    fixture
        .run(&repo, |r| {
            conflicts::accept(r, &["a.txt".to_string()], Side::Yours)
        })
        .unwrap();
    let went = trace.of(
        "Continue Rebase after resolving",
        &fixture,
        &repo,
        rebase::continue_rebase,
    );
    checked("continue", went.is_ok());

    let repo = Scratch::diverged();
    repo.git(&["checkout", "-q", "side"]);
    let head = repo.head();
    fixture
        .run(&repo, {
            let params = rebasing("side");
            move |r| rebase::rebase(r, &params, &head)
        })
        .unwrap();
    let skipped = trace.of("Skip Commit", &fixture, &repo, rebase::skip);
    checked("skip", skipped.is_ok());

    let repo = Scratch::diverged();
    repo.git(&["checkout", "-q", "side"]);
    let head = repo.head();
    fixture
        .run(&repo, {
            let params = rebasing("side");
            move |r| rebase::rebase(r, &params, &head)
        })
        .unwrap();
    let aborted = trace.of("Abort Rebase", &fixture, &repo, rebase::abort);
    checked("abort rebase", aborted.is_ok());

    // Interactive rebase from the log, with the dialog's plan: squash and reword.
    let repo = Scratch::new();
    repo.commit("base.txt", "base\n", "base");
    let first = repo.commit("A.txt", "A\n", "A");
    repo.commit("B.txt", "B\n", "B");
    repo.commit("C.txt", "C\n", "C");
    repo.commit("D.txt", "D\n", "D");
    let (start, head) = (first.clone(), repo.head());
    let entries = fixture
        .run(&repo, move |r| rewrite::interactive_entries(r, &start))
        .unwrap()
        .expect("the range must be linear");
    let plan = planned(
        entries,
        &[
            ("B", Action::Squash),
            ("C", Action::Reword),
            ("D", Action::Drop),
        ],
        &[("C", "C rewritten")],
    );
    let done = trace.of(
        "interactive rebase (squash, reword and drop from the dialog)",
        &fixture,
        &repo,
        move |r| rewrite::interactive(r, &first, Some(plan), &head),
    );
    checked("interactive", done.is_ok());

    // Cherry-pick and revert: without conflict, with conflict + commit, and abort.
    let repo = Scratch::new();
    repo.commit("a.txt", "base\n", "base");
    repo.git(&["checkout", "-q", "-b", "side"]);
    let clean = repo.commit("clean.txt", "l\n", "clean");
    let clashing = repo.commit("a.txt", "side\n", "clashing");
    repo.git(&["checkout", "-q", "main"]);
    repo.commit("a.txt", "main\n", "main");
    let head = repo.head();
    let picked = trace.of("cherry-pick without conflict", &fixture, &repo, {
        let (hash, head) = (clean.clone(), head.clone());
        move |r| apply_commits::run(r, Kind::CherryPick, &[hash], &head)
    });
    checked("cherry-pick", picked.is_ok());
    let head = repo.head();
    let picked = trace.of("cherry-pick with conflict", &fixture, &repo, {
        let (hash, head) = (clashing.clone(), head.clone());
        move |r| apply_commits::run(r, Kind::CherryPick, &[hash], &head)
    });
    checked("cherry-pick with conflict", picked.is_ok());
    fixture
        .run(&repo, |r| {
            conflicts::accept(r, &["a.txt".to_string()], Side::Theirs)
        })
        .unwrap();
    let finished = trace.of("commit the cherry-pick after resolving", &fixture, &repo, {
        let hash = clashing.clone();
        move |r| apply_commits::commit_after_conflicts(r, Kind::CherryPick, &hash)
    });
    checked("commit after cherry-pick", finished.is_ok());

    let repo = Scratch::new();
    repo.commit("a.txt", "base\n", "base");
    let changing = repo.commit("a.txt", "one\n", "changes a");
    repo.commit("a.txt", "two\n", "changes a again");
    let head = repo.head();
    let reverted = trace.of("revert with conflict", &fixture, &repo, {
        let (hash, head) = (changing.clone(), head.clone());
        move |r| apply_commits::run(r, Kind::Revert, &[hash], &head)
    });
    checked("revert with conflict", reverted.is_ok());
    let aborted = trace.of("Abort Revert", &fixture, &repo, |r| {
        operation::abort(r, Operation::Reverting)
    });
    checked("abort revert", aborted.is_ok());

    let repo = Scratch::new();
    repo.commit("a.txt", "base\n", "base");
    let added = repo.commit("n.txt", "n\n", "added");
    repo.commit("b.txt", "b\n", "another");
    let head = repo.head();
    let reverted = trace.of("revert without conflict", &fixture, &repo, {
        let (hash, head) = (added.clone(), head.clone());
        move |r| apply_commits::run(r, Kind::Revert, &[hash], &head)
    });
    checked("revert", reverted.is_ok());

    // Cherry-Pick Selected Changes and Revert Selected Changes, with git apply.
    let repo = Scratch::new();
    repo.commit("a.txt", "1\n", "base");
    let change = repo.commit("a.txt", "1\n2\n", "adds 2");
    repo.git(&["checkout", "-q", "-b", "other", "HEAD~1"]);
    let head = repo.head();
    let selection = vec![Selection {
        hash: change.clone(),
        base: Some(format!("{change}^")),
        paths: vec!["a.txt".to_string()],
    }];
    let applied = trace.of("Cherry-Pick Selected Changes", &fixture, &repo, {
        let (selection, head) = (selection.clone(), head.clone());
        move |r| apply_commits::apply_selection(r, &head, &selection, false)
    });
    checked("apply selection", applied.is_ok());
    let reverted = trace.of("Revert Selected Changes", &fixture, &repo, {
        let head = head.clone();
        move |r| apply_commits::apply_selection(r, &head, &selection, true)
    });
    checked("revert selection", reverted.is_ok());

    // Undo commit, reword (HEAD and an earlier one), drop and squash.
    let five = || {
        let repo = Scratch::new();
        for subject in ["base", "A", "B", "C", "D"] {
            repo.commit(&format!("{subject}.txt"), subject, subject);
        }
        let hashes: Vec<String> = repo
            .git(&["log", "--format=%H"])
            .lines()
            .map(str::to_string)
            .collect();
        (repo, hashes)
    };
    // `hashes` goes from the most recent to the oldest: D, C, B, A, base.
    let (repo, hashes) = five();
    let (tip, head) = (hashes[0].clone(), repo.head());
    let undone = trace.of("undo commit", &fixture, &repo, move |r| {
        rewrite::uncommit(r, &tip, &head)
    });
    checked("uncommit", undone.is_ok());

    let (repo, hashes) = five();
    let (tip, head) = (hashes[0].clone(), repo.head());
    let reworded = trace.of(
        "reword of the last commit (amend)",
        &fixture,
        &repo,
        move |r| rewrite::reword(r, &tip, "D new", &head),
    );
    checked("reword HEAD", reworded.is_ok());

    let (repo, hashes) = five();
    let (target, head) = (hashes[2].clone(), repo.head());
    let reworded = trace.of(
        "reword of an earlier commit (rebase)",
        &fixture,
        &repo,
        move |r| rewrite::reword(r, &target, "B new", &head),
    );
    checked("earlier reword", reworded.is_ok());

    let (repo, hashes) = five();
    let (dropped, head) = (vec![hashes[2].clone()], repo.head());
    let dropped = trace.of("drop", &fixture, &repo, move |r| {
        rewrite::drop(r, &dropped, &head)
    });
    checked("drop", dropped.is_ok());

    let (repo, hashes) = five();
    let (picked, head) = (vec![hashes[1].clone(), hashes[2].clone()], repo.head());
    let squashed = trace.of("squash", &fixture, &repo, move |r| {
        rewrite::squash(r, &picked, "B and C together", &head)
    });
    checked("squash", squashed.is_ok());

    // Interactive rebase that stops at a commit marked `edit`, and its Continue.
    let (repo, hashes) = five();
    let (start, head) = (hashes[3].clone(), repo.head());
    let entries = fixture
        .run(&repo, {
            let start = start.clone();
            move |r| rewrite::interactive_entries(r, &start)
        })
        .unwrap()
        .expect("linear");
    let plan = planned(entries, &[("B", Action::Edit)], &[]);
    let done = trace.of(
        "interactive rebase with edit (stops)",
        &fixture,
        &repo,
        move |r| rewrite::interactive(r, &start, Some(plan), &head),
    );
    checked("interactive edit", done.is_ok());
    let went = trace.of(
        "Continue Rebase after edit",
        &fixture,
        &repo,
        rebase::continue_rebase,
    );
    checked("continue edit", went.is_ok());

    trace.silence();
}
