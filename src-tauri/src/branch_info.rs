// What the branch popup and panel show: the branches, the recent ones, the favorites, and which
// branches have commits to fetch or to push. None of this writes to the repository.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::branches::{self, ResetMode};
use crate::git::{self, GitError, PATHS_PER_COMMAND};
use crate::settings::IncomingCheck;
use crate::storage;
use crate::task::{Runner, TaskContext};

const HEADS: &str = "refs/heads/";
const REMOTES: &str = "refs/remotes/";
const REF_FORMAT: &str = "--format=%(refname)%09%(objectname)%09%(symref)%09%(upstream)%09%(push)\
                          %09%(upstream:remotename)%09%(upstream:remoteref)";
const REF_FIELDS: usize = 7;
// The registry keys `git.recent.checkout.branches.reflog.entries.count` and
// `git.show.recent.checkout.branches`.
const REFLOG_ENTRIES: &str = "50";
const RECENT_BRANCHES: usize = 5;
const CHECKOUT_FROM: &str = ": moving from ";
const CHECKOUT_TO: &str = " to ";
// `GitBranchIncomingOutgoingManager.MAC_DEFAULT_LAUNCH`.
const SYSTEM_AGENT: &str = "com.apple.launchd";
const HTTP: &str = "http";
// The registry key `git.update.incoming.info.time`.
const CHECK_PERIOD: Duration = Duration::from_secs(20 * 60);
const FAVORITES_FILE: &str = "branch-favorites.json";
const OPTIONS_FILE: &str = "branch-options.json";
const RESET_MODE_FILE: &str = "reset-mode.json";
// `GitBranchManager.getDefaultBranchNames`.
const DEFAULT_LOCAL: [&str; 2] = ["main", "master"];
const DEFAULT_REMOTE: [&str; 2] = ["origin/main", "origin/master"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalBranch {
    pub name: String,
    // Without `refs/remotes/`, the way the popup shows it.
    pub tracked: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchList {
    pub local: Vec<LocalBranch>,
    pub remote: Vec<String>,
    pub recent: Vec<String>,
    pub current: Option<String>,
    // `recentBranchesByRepository`: where the last checkout came from. IntelliJ saves it when
    // doing it; here it comes from the reflog, which also sees the ones made outside the app.
    pub previous: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Sync {
    pub incoming: bool,
    pub outgoing: bool,
}

// `myLocalBranchesToFetch`: for each local branch the remote has moved, the hash of its remote
// branch as it is known here. It holds until a fetch changes it.
pub type ToFetch = HashMap<String, String>;

struct Ref {
    name: String,
    hash: String,
    symbolic: bool,
    upstream: Option<String>,
    push: Option<String>,
    // The remote and the branch in it, if `upstream` really is from a remote.
    target: Option<(String, String)>,
}

fn filled(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_string())
}

fn parse_ref(line: &str) -> Option<Ref> {
    let fields: Vec<&str> = line.split('\t').collect();
    if fields.len() != REF_FIELDS {
        return None;
    }
    let upstream = filled(fields[3]);
    let from_remote = upstream
        .as_deref()
        .is_some_and(|name| name.starts_with(REMOTES));
    Some(Ref {
        name: fields[0].to_string(),
        hash: fields[1].to_string(),
        symbolic: !fields[2].is_empty(),
        upstream,
        push: filled(fields[4]),
        target: (from_remote && !fields[5].is_empty() && !fields[6].is_empty())
            .then(|| (fields[5].to_string(), fields[6].to_string())),
    })
}

fn parse_refs(output: &str) -> Vec<Ref> {
    output.lines().filter_map(parse_ref).collect()
}

fn read_refs(repo: &Path) -> Result<Vec<Ref>, GitError> {
    let args = ["for-each-ref", REF_FORMAT, "refs/heads", "refs/remotes"];
    let output = git::run(repo, &args)?;
    Ok(parse_refs(&String::from_utf8_lossy(&output)))
}

// `moving from a to b`: `a` if there is one, and `b`, the text after the last ` to `.
fn parse_checkout(line: &str) -> Option<(Option<&str>, &str)> {
    let to = line.rfind(CHECKOUT_TO).filter(|index| *index > 0)?;
    let name = &line[to + CHECKOUT_TO.len()..];
    let from = line[..to]
        .find(CHECKOUT_FROM)
        .map(|start| &line[start + CHECKOUT_FROM.len()..to]);
    Some((from, name))
}

// `collectRecentCheckoutBranches`. If the reflog cannot be read, there are no recent ones.
fn checkouts(repo: &Path) -> Vec<String> {
    let args = [
        "reflog",
        "--max-count",
        REFLOG_ENTRIES,
        "--grep-reflog",
        "checkout:",
        "--",
    ];
    git::run(repo, &args)
        .map(|output| {
            String::from_utf8_lossy(&output)
                .lines()
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

fn recent_of(lines: &[String], has_local: impl Fn(&str) -> bool) -> (Vec<String>, Option<String>) {
    let moves: Vec<_> = lines
        .iter()
        .filter_map(|line| parse_checkout(line))
        .collect();
    let previous = moves
        .first()
        .and_then(|(from, _)| *from)
        .filter(|from| has_local(from))
        .map(String::from);
    let mut recent: Vec<String> = Vec::new();
    let names = moves.iter().map(|(_, name)| *name);
    for name in names.chain(previous.as_deref()) {
        if has_local(name) && !recent.iter().any(|known| known == name) {
            recent.push(name.to_string());
        }
    }
    recent.truncate(RECENT_BRANCHES);
    (recent, previous)
}

pub fn list(repo: &Path) -> Result<BranchList, GitError> {
    let refs = read_refs(repo)?;
    let current = branches::current_name(repo)?;
    let mut local = Vec::new();
    let mut remote = Vec::new();
    for reference in &refs {
        if let Some(name) = reference.name.strip_prefix(HEADS) {
            local.push(LocalBranch {
                name: name.to_string(),
                tracked: reference
                    .upstream
                    .as_deref()
                    .and_then(|upstream| upstream.strip_prefix(REMOTES))
                    .map(String::from),
            });
        } else if let Some(name) = reference.name.strip_prefix(REMOTES) {
            if !reference.symbolic {
                remote.push(name.to_string());
            }
        }
    }
    // `localBranchesOrCurrent`: before the first commit the branch does not exist yet as a reference.
    if let (true, Some(name)) = (local.is_empty(), &current) {
        local.push(LocalBranch {
            name: name.clone(),
            tracked: None,
        });
    }
    let (recent, previous) = recent_of(&checkouts(repo), |name| {
        local.iter().any(|branch| branch.name == name)
    });
    Ok(BranchList {
        local,
        remote,
        recent,
        current,
        previous,
    })
}

// `hasCommitsForBranch`: if git fails, it is assumed there is nothing.
fn has_commits(repo: &Path, range: &str) -> bool {
    git::run(repo, &["rev-list", "--count", range, "--"])
        .is_ok_and(|output| !String::from_utf8_lossy(&output).starts_with('0'))
}

// `GitBranchIncomingOutgoingManager`. `%(push)` gives the target of `GitPushTarget`: the remote's
// refspec, or the tracked branch if it is the push one. Otherwise it comes out empty and the tracked
// one applies. With hashes, a tag named like the branch does not confuse.
pub fn incoming_outgoing(
    repo: &Path,
    strategy: IncomingCheck,
    to_fetch: &ToFetch,
) -> Result<HashMap<String, Sync>, GitError> {
    let refs = read_refs(repo)?;
    let hashes = hashes_of(&refs);
    let mut result = HashMap::new();
    for reference in &refs {
        let Some(name) = reference.name.strip_prefix(HEADS) else {
            continue;
        };
        let hash_of =
            |other: &Option<String>| other.as_deref().and_then(|o| hashes.get(o).copied());
        let tracked = hash_of(&reference.upstream);
        let pushed = hash_of(&reference.push).or(tracked);
        let outgoing = pushed.is_some_and(|hash| {
            hash != reference.hash && has_commits(repo, &format!("{hash}..{}", reference.hash))
        });
        let incoming = strategy != IncomingCheck::Never
            && tracked.is_some_and(|hash| {
                let behind = hash != reference.hash
                    && has_commits(repo, &format!("{}..{hash}", reference.hash));
                behind || to_fetch.get(name).is_some_and(|seen| seen == hash)
            });
        if incoming || outgoing {
            result.insert(name.to_string(), Sync { incoming, outgoing });
        }
    }
    Ok(result)
}

// `hasExternalSSHAgent`: no agent is not external; a launchd one is external only if its path is not
// the canonical one (a symlink, or a symlinked ancestor) and any failure to find out means it is not;
// any other agent is external.
fn has_external_agent(agent: Option<&OsStr>) -> bool {
    let Some(agent) = agent else { return false };
    if !agent.to_string_lossy().contains(SYSTEM_AGENT) {
        return true;
    }
    fs::canonicalize(agent).is_ok_and(|real| real.as_os_str() != agent)
}

// `shouldAvoidUserInteraction`: another agent could ask on its own, outside our askpass. For
// `containsSSHUrl`, ssh is everything that does not start with http.
fn avoids_interaction(strategy: IncomingCheck, agent: Option<&OsStr>, urls: &[String]) -> bool {
    strategy == IncomingCheck::Auto
        && has_external_agent(agent)
        && urls.iter().any(|url| !url.starts_with(HTTP))
}

fn remote_urls(ctx: &TaskContext, remote: &str) -> Vec<String> {
    ctx.git(&["remote", "get-url", "--all", "--", remote])
        .map(|output| output.stdout.lines().map(String::from).collect())
        .unwrap_or_default()
}

fn parse_ls_remote(output: &str) -> HashMap<&str, &str> {
    output
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(hash, name)| (name, hash))
        .collect()
}

// `calcBranchesToFetchForRemote`. A remote that fails says nothing; what it already said stays.
fn check_remote(
    ctx: &TaskContext,
    sync: &BranchSync,
    remote: &str,
    branches: &[(&Ref, &str)],
    hashes: &HashMap<&str, &str>,
    found: &mut ToFetch,
) {
    for chunk in branches.chunks(PATHS_PER_COMMAND) {
        let mut args = vec!["ls-remote", "--heads", "--", remote];
        args.extend(chunk.iter().map(|(_, remote_ref)| *remote_ref));
        let Ok(output) = ctx.git(&args) else {
            return;
        };
        sync.mark_authenticated(ctx.repo(), remote);
        let heads = parse_ls_remote(&output.stdout);
        for (branch, remote_ref) in chunk {
            let known = branch.upstream.as_deref().and_then(|name| hashes.get(name));
            if let (Some(known), Some(head), Some(name)) = (
                known,
                heads.get(remote_ref),
                branch.name.strip_prefix(HEADS),
            ) {
                if known != head {
                    found.insert(name.to_string(), known.to_string());
                }
            }
        }
    }
}

fn hashes_of(refs: &[Ref]) -> HashMap<&str, &str> {
    refs.iter()
        .map(|reference| (reference.name.as_str(), reference.hash.as_str()))
        .collect()
}

fn tracked_by_remote(refs: &[Ref]) -> BTreeMap<&str, Vec<(&Ref, &str)>> {
    let mut by_remote: BTreeMap<&str, Vec<(&Ref, &str)>> = BTreeMap::new();
    for reference in refs.iter().filter(|r| r.name.starts_with(HEADS)) {
        if let Some((remote, remote_ref)) = &reference.target {
            by_remote
                .entry(remote)
                .or_default()
                .push((reference, remote_ref));
        }
    }
    by_remote
}

fn check_remotes_with(
    runner: &Runner,
    repo: &Path,
    strategy: IncomingCheck,
    sync: &BranchSync,
    limit: Duration,
    agent: Option<&OsStr>,
) -> Result<ToFetch, GitError> {
    let mut found = ToFetch::new();
    if strategy == IncomingCheck::Never {
        return Ok(found);
    }
    let refs = read_refs(repo)?;
    let hashes = hashes_of(&refs);
    for (remote, branches) in tracked_by_remote(&refs) {
        // `getAuthenticationMode`: `NONE` for whoever has not authenticated, and `Always` does not change it.
        // Opening a repository must not wake the keychain or launch anyone's `core.sshCommand`.
        if !sync.is_authenticated(repo, remote) {
            continue;
        }
        // Going over the deadline only cuts this remote: what was found stays in `found`.
        let _ = runner.quiet(repo, limit, |ctx| {
            if !avoids_interaction(strategy, agent, &remote_urls(ctx, remote)) {
                check_remote(ctx, sync, remote, &branches, &hashes, &mut found);
            }
            Ok(())
        });
    }
    Ok(found)
}

// `limit` applies to each remote: a hung one does not take the others' results down with it.
pub fn check_remotes(
    runner: &Runner,
    repo: &Path,
    strategy: IncomingCheck,
    sync: &BranchSync,
    limit: Duration,
) -> Result<ToFetch, GitError> {
    let agent = env::var_os("SSH_AUTH_SOCK");
    check_remotes_with(runner, repo, strategy, sync, limit, agent.as_deref())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BranchKind {
    Local,
    Remote,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FavoriteSet {
    pub local: Vec<String>,
    pub remote: Vec<String>,
}

impl FavoriteSet {
    fn of(&mut self, kind: BranchKind) -> &mut Vec<String> {
        match kind {
            BranchKind::Local => &mut self.local,
            BranchKind::Remote => &mut self.remote,
        }
    }
}

// `Git.Settings` stores the same: the chosen ones and the default ones that were removed.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct StoredFavorites {
    favorites: FavoriteSet,
    excluded: FavoriteSet,
}

const fn defaults_of(kind: BranchKind) -> [&'static str; 2] {
    match kind {
        BranchKind::Local => DEFAULT_LOCAL,
        BranchKind::Remote => DEFAULT_REMOTE,
    }
}

fn effective(stored: &mut StoredFavorites, kind: BranchKind) -> Vec<String> {
    let excluded = stored.excluded.of(kind).clone();
    let mut names = stored.favorites.of(kind).clone();
    names.extend(
        defaults_of(kind)
            .iter()
            .filter(|name| !excluded.iter().any(|other| other == *name))
            .map(|name| name.to_string()),
    );
    names.sort();
    names.dedup();
    names
}

fn resolved(mut stored: StoredFavorites) -> FavoriteSet {
    FavoriteSet {
        local: effective(&mut stored, BranchKind::Local),
        remote: effective(&mut stored, BranchKind::Remote),
    }
}

// A damaged file is set aside: fewer favorites, but without losing what was there.
fn read_favorites(dir: &Path) -> StoredFavorites {
    let file = dir.join(FAVORITES_FILE);
    let Ok(bytes) = fs::read(&file) else {
        return StoredFavorites::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_else(|_| {
        let _ = storage::keep_corrupt(dir, &file, &format!("{FAVORITES_FILE}.corrupt"));
        StoredFavorites::default()
    })
}

pub fn favorites(dir: &Path) -> FavoriteSet {
    resolved(read_favorites(dir))
}

fn remove(names: &mut Vec<String>, name: &str) {
    names.retain(|other| other != name);
}

// `DvcsBranchManager.setFavorite`: removing a default one leaves an exclusion.
pub fn toggle_favorite(dir: &Path, name: &str, kind: BranchKind) -> io::Result<FavoriteSet> {
    let mut stored = read_favorites(dir);
    let is_default = defaults_of(kind).contains(&name);
    if effective(&mut stored, kind)
        .iter()
        .any(|other| other == name)
    {
        remove(stored.favorites.of(kind), name);
        if is_default {
            stored.excluded.of(kind).push(name.to_string());
        }
    } else {
        remove(stored.excluded.of(kind), name);
        if !is_default {
            stored.favorites.of(kind).push(name.to_string());
        }
    }
    fs::create_dir_all(dir)?;
    let json = serde_json::to_vec_pretty(&stored).map_err(io::Error::other)?;
    storage::write_atomic(&dir.join(FAVORITES_FILE), &json)?;
    Ok(resolved(stored))
}

// The popup and the panel remember their state per repository. The interface knows the shape.
pub fn read_options(dir: &Path) -> serde_json::Value {
    fs::read(dir.join(OPTIONS_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

pub fn write_options(dir: &Path, options: &serde_json::Value) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let json = serde_json::to_vec_pretty(options).map_err(io::Error::other)?;
    storage::write_atomic(&dir.join(OPTIONS_FILE), &json)
}

// `GitResetDialog` remembers the last mode; here, per repository.
pub fn read_reset_mode(dir: &Path) -> Option<ResetMode> {
    let bytes = fs::read(dir.join(RESET_MODE_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn write_reset_mode(dir: &Path, mode: ResetMode) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let json = serde_json::to_vec(&mode).map_err(io::Error::other)?;
    storage::write_atomic(&dir.join(RESET_MODE_FILE), &json)
}

// When it is time to ask the remotes. With the app hidden there is no timer: what is pending waits
// until it is visible again, and an idle repository only wakes this every 20 minutes.
#[derive(Debug, Default)]
struct Schedule {
    pending: bool,
    next: Option<Instant>,
}

impl Schedule {
    fn request(&mut self) {
        self.pending = true;
    }

    fn should_run(&mut self, visible: bool, now: Instant) -> bool {
        if !self.pending || !visible {
            return false;
        }
        self.pending = false;
        self.next = Some(now + CHECK_PERIOD);
        true
    }

    fn wait(&self, now: Instant) -> Option<Duration> {
        if self.pending {
            return None;
        }
        self.next.map(|next| next.saturating_duration_since(now))
    }
}

enum Wake {
    Request,
    Visible,
}

fn schedule_loop(wakes: Receiver<Wake>, visible: impl Fn() -> bool, mut check: impl FnMut()) {
    let mut schedule = Schedule::default();
    loop {
        let received = match schedule.wait(Instant::now()) {
            Some(wait) => wakes.recv_timeout(wait),
            None => wakes.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match received {
            Ok(Wake::Request) | Err(RecvTimeoutError::Timeout) => schedule.request(),
            Ok(Wake::Visible) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        if schedule.should_run(visible(), Instant::now()) {
            check();
        }
    }
}

// The background check and the last thing it saw.
#[derive(Default)]
pub struct BranchSync {
    wake: Mutex<Option<Sender<Wake>>>,
    seen: Mutex<Option<(PathBuf, ToFetch)>>,
    // `myAuthSuccessMap`: only valid in this session.
    authenticated: Mutex<HashSet<(PathBuf, String)>>,
}

impl BranchSync {
    // The thread only exists to wait: `check` runs on it, and they do not overlap.
    pub fn start(
        &self,
        visible: impl Fn() -> bool + Send + 'static,
        check: impl FnMut() + Send + 'static,
    ) {
        let (send, wakes) = mpsc::channel();
        *self.wake.lock().unwrap_or_else(PoisonError::into_inner) = Some(send);
        std::thread::spawn(move || schedule_loop(wakes, visible, check));
    }

    fn send(&self, wake: Wake) {
        let sender = self.wake.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(sender) = sender.as_ref() {
            let _ = sender.send(wake);
        }
    }

    pub fn request(&self) {
        self.send(Wake::Request);
    }

    pub fn visible(&self) {
        self.send(Wake::Visible);
    }

    // Called by whoever finishes an operation with that remote successfully: fetch, push or ls-remote.
    pub fn mark_authenticated(&self, repo: &Path, remote: &str) {
        self.authenticated
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert((repo.to_path_buf(), remote.to_string()));
    }

    pub(crate) fn is_authenticated(&self, repo: &Path, remote: &str) -> bool {
        self.authenticated
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&(repo.to_path_buf(), remote.to_string()))
    }

    pub fn store(&self, repo: &Path, found: ToFetch) {
        *self.seen.lock().unwrap_or_else(PoisonError::into_inner) =
            Some((repo.to_path_buf(), found));
    }

    pub fn seen(&self, repo: &Path) -> ToFetch {
        let seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        match seen.as_ref() {
            Some((path, found)) if path == repo => found.clone(),
            _ => ToFetch::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::Runner;
    use crate::test_support::TempRepo;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tempfile::TempDir;

    // A bare remote and a clone of it, both on `main`.
    struct Remote {
        bare: TempRepo,
        clone: TempRepo,
    }

    fn remote() -> Remote {
        let bare = TempRepo::new();
        bare.git(&["config", "core.bare", "true"]);
        let source = TempRepo::new();
        source.commit("a.txt", b"1", "one");
        source.git(&["push", "-q", path(&bare), "main"]);
        let clone = TempRepo::new();
        clone.git(&["remote", "add", "origin", &url(&bare)]);
        clone.git(&["fetch", "-q", "origin"]);
        clone.git(&["checkout", "-q", "-b", "main", "origin/main"]);
        Remote { bare, clone }
    }

    fn path(repo: &TempRepo) -> &str {
        repo.path().to_str().unwrap()
    }

    fn url(repo: &TempRepo) -> String {
        format!("file://{}", path(repo))
    }

    // Another clone pushes a commit: the remote moves ahead without ours fetching.
    fn advance(bare: &TempRepo) {
        let other = TempRepo::new();
        other.git(&["remote", "add", "origin", &url(bare)]);
        other.git(&["fetch", "-q", "origin"]);
        other.git(&["checkout", "-q", "-b", "main", "origin/main"]);
        other.commit("b.txt", b"2", "two");
        other.git(&["push", "-q", "origin", "main"]);
    }

    fn runner() -> Runner {
        Runner::new(Arc::default(), Arc::default())
    }

    fn runner_with_grace() -> Runner {
        Runner::with(
            Arc::default(),
            Arc::default(),
            None,
            Duration::from_millis(100),
        )
    }

    fn check(
        repo: &TempRepo,
        sync: &BranchSync,
        strategy: IncomingCheck,
        agent: Option<&str>,
    ) -> ToFetch {
        let agent = agent.map(OsStr::new);
        check_remotes_with(&runner(), repo.path(), strategy, sync, LIMIT, agent).unwrap()
    }

    // All its remotes were already authenticated in this session.
    fn authenticated(repo: &TempRepo) -> BranchSync {
        let sync = BranchSync::default();
        for remote in repo.git(&["remote"]).lines() {
            sync.mark_authenticated(repo.path(), remote);
        }
        sync
    }

    fn checked(repo: &TempRepo, strategy: IncomingCheck, agent: Option<&str>) -> ToFetch {
        check(repo, &authenticated(repo), strategy, agent)
    }

    const LIMIT: Duration = Duration::from_secs(30);
    const NO_AGENT: Option<&str> = None;
    const SYSTEM_AGENT: Option<&str> = Some("/private/tmp/com.apple.launchd.abc/Listeners");
    const OTHER_AGENT: Option<&str> = Some("/Users/me/.1password/agent.sock");

    #[test]
    fn lists_branches_and_tracking() {
        let Remote { clone, .. } = remote();
        clone.git(&["branch", "-q", "--no-track", "alt"]);
        clone.git(&["remote", "set-head", "origin", "main"]);

        let list = list(clone.path()).unwrap();

        assert_eq!(
            list.local,
            [
                LocalBranch {
                    name: "alt".to_string(),
                    tracked: None
                },
                LocalBranch {
                    name: "main".to_string(),
                    tracked: Some("origin/main".to_string())
                },
            ]
        );
        assert_eq!(list.remote, ["origin/main"], "without origin/HEAD");
        assert_eq!(list.current.as_deref(), Some("main"));
    }

    #[test]
    fn without_commits_the_current_branch_counts_as_local() {
        let repo = TempRepo::new();

        let list = list(repo.path()).unwrap();

        assert_eq!(list.local.len(), 1);
        assert_eq!(list.local[0].name, "main");
        assert!(list.recent.is_empty());
        assert_eq!(list.previous, None);
    }

    #[test]
    fn recent_ones_come_from_the_reflog() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        for name in ["a", "b", "c", "d", "e", "f"] {
            repo.git(&["checkout", "-q", "-b", name]);
        }
        repo.git(&["checkout", "-q", "c"]);
        repo.git(&["branch", "-q", "-D", "e"]);
        repo.git(&["checkout", "-q", "--detach"]);
        repo.git(&["checkout", "-q", "c"]);

        let list = list(repo.path()).unwrap();

        // From newest to oldest, without repeats, without the deleted one, without the hash and at most 5.
        assert_eq!(list.recent, ["c", "f", "d", "b", "a"]);
    }

    #[test]
    fn the_previous_one_is_where_the_last_checkout_came_from() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        repo.git(&["checkout", "-q", "-b", "side"]);
        repo.git(&["checkout", "-q", "main"]);

        assert_eq!(list(repo.path()).unwrap().previous.as_deref(), Some("side"));
    }

    #[test]
    fn after_cloning_the_initial_branch_is_only_there_as_previous() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        repo.git(&["checkout", "-q", "-b", "side"]);

        let list = list(repo.path()).unwrap();

        assert_eq!(list.previous.as_deref(), Some("main"));
        assert_eq!(list.recent, ["side", "main"]);
    }

    #[test]
    fn the_previous_one_deleted_or_detached_does_not_count() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "one");
        repo.git(&["checkout", "-q", "--detach"]);
        repo.git(&["checkout", "-q", "main"]);

        assert_eq!(list(repo.path()).unwrap().previous, None);
    }

    #[test]
    fn outgoing_with_rev_list() {
        let Remote { clone, .. } = remote();
        clone.commit("b.txt", b"2", "local");

        let sync = incoming_outgoing(clone.path(), IncomingCheck::Auto, &ToFetch::new()).unwrap();

        assert_eq!(
            sync["main"],
            Sync {
                incoming: false,
                outgoing: true
            }
        );
    }

    #[test]
    fn incoming_with_rev_list() {
        let Remote { bare, clone } = remote();
        advance(&bare);
        clone.git(&["fetch", "-q", "origin"]);

        let sync = incoming_outgoing(clone.path(), IncomingCheck::Auto, &ToFetch::new()).unwrap();

        assert_eq!(
            sync["main"],
            Sync {
                incoming: true,
                outgoing: false
            }
        );
    }

    #[test]
    fn up_to_date_and_untracked_do_not_appear() {
        let Remote { clone, .. } = remote();
        clone.git(&["branch", "-q", "--no-track", "side"]);

        let sync = incoming_outgoing(clone.path(), IncomingCheck::Auto, &ToFetch::new()).unwrap();

        assert!(sync.is_empty(), "{sync:?}");
    }

    #[test]
    fn a_tag_named_like_the_branch_does_not_confuse() {
        let Remote { clone, .. } = remote();
        let first = clone.git(&["rev-parse", "HEAD"]);
        clone.commit("b.txt", b"2", "local");
        clone.git(&["tag", "main", first.trim()]);

        let sync = incoming_outgoing(clone.path(), IncomingCheck::Auto, &ToFetch::new()).unwrap();

        assert!(sync["main"].outgoing);
    }

    #[test]
    fn ls_remote_detects_commits_without_fetch() {
        let Remote { bare, clone } = remote();
        let before = clone.git(&["rev-parse", "origin/main"]);
        advance(&bare);

        let to_fetch = checked(&clone, IncomingCheck::Always, NO_AGENT);

        assert_eq!(to_fetch["main"], before.trim());
        let sync = incoming_outgoing(clone.path(), IncomingCheck::Auto, &to_fetch).unwrap();
        assert!(sync["main"].incoming);
    }

    #[test]
    fn after_the_fetch_what_ls_remote_saw_stops_counting() {
        let Remote { bare, clone } = remote();
        advance(&bare);
        let to_fetch = checked(&clone, IncomingCheck::Always, NO_AGENT);
        clone.git(&["fetch", "-q", "origin"]);
        clone.git(&["merge", "-q", "--ff-only", "origin/main"]);

        let sync = incoming_outgoing(clone.path(), IncomingCheck::Auto, &to_fetch).unwrap();

        assert!(sync.is_empty(), "{sync:?}");
    }

    #[test]
    fn ls_remote_does_not_confuse_another_branch_with_the_same_ending() {
        let Remote { bare, clone } = remote();
        let other = TempRepo::new();
        other.commit("x.txt", b"x", "another");
        other.git(&["push", "-q", path(&bare), "main:refs/heads/side/main"]);

        assert!(checked(&clone, IncomingCheck::Always, NO_AGENT).is_empty());
    }

    #[test]
    fn auto_skips_ssh_if_the_agent_is_not_the_system_one() {
        let Remote { bare, clone } = remote();
        advance(&bare);

        // For `containsSSHUrl`, everything that does not start with http is ssh: also `file://`.
        assert!(checked(&clone, IncomingCheck::Auto, OTHER_AGENT).is_empty());
        assert!(checked(&clone, IncomingCheck::Auto, SYSTEM_AGENT).contains_key("main"));
        assert!(checked(&clone, IncomingCheck::Auto, NO_AGENT).contains_key("main"));
        assert!(checked(&clone, IncomingCheck::Always, OTHER_AGENT).contains_key("main"));
    }

    #[test]
    fn a_symlinked_launchd_agent_counts_as_external() {
        let dir = TempDir::new().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let launchd = root.join("com.apple.launchd.x");
        fs::create_dir(&launchd).unwrap();
        let target = root.join("elsewhere");
        fs::write(&target, "").unwrap();
        let link = launchd.join("Listeners");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        // A real file under a launchd name is the system one.
        let plain = launchd.join("Plain");
        fs::write(&plain, "").unwrap();

        assert!(has_external_agent(Some(link.as_os_str())));
        assert!(!has_external_agent(Some(plain.as_os_str())));
        // Missing: the check fails and that counts as not external.
        assert!(!has_external_agent(Some(OsStr::new("/private/tmp/com.apple.launchd.nope/Listeners"))));
        assert!(!has_external_agent(None));
        assert!(has_external_agent(Some(OsStr::new("/Users/me/.1password/agent.sock"))));
    }

    #[test]
    fn auto_no_salta_https() {
        let urls = ["https://example.com/r.git".to_string()];
        let ssh = ["git@example.com:r.git".to_string()];
        let other = OTHER_AGENT.map(OsStr::new);

        assert!(!avoids_interaction(IncomingCheck::Auto, other, &urls));
        assert!(avoids_interaction(IncomingCheck::Auto, other, &ssh));
    }

    // An ssh remote whose client is a script: it leaves a trace and fails, without network.
    fn traced_remote(clone: &TempRepo, trace: &TempDir) -> PathBuf {
        let log = trace.path().join("command");
        let script = trace.path().join("ssh");
        fs::write(&script, format!("#!/bin/sh\necho x >> {log:?}\nexit 1\n")).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        clone.git(&["config", "core.sshCommand", script.to_str().unwrap()]);
        clone.git(&["remote", "set-url", "origin", "ssh://me@127.0.0.1/r.git"]);
        log
    }

    #[test]
    fn never_does_not_launch_ls_remote() {
        let Remote { clone, .. } = remote();
        let trace = TempDir::new().unwrap();
        let log = traced_remote(&clone, &trace);

        assert!(checked(&clone, IncomingCheck::Never, NO_AGENT).is_empty());
        assert!(!log.exists(), "ls-remote with Never");

        assert!(checked(&clone, IncomingCheck::Always, NO_AGENT).is_empty());
        assert!(
            log.exists(),
            "the test must see the remote when it is used"
        );
    }

    #[test]
    fn without_prior_authentication_it_does_not_launch_ls_remote() {
        let Remote { clone, .. } = remote();
        let trace = TempDir::new().unwrap();
        let log = traced_remote(&clone, &trace);
        let sync = BranchSync::default();

        for strategy in [IncomingCheck::Auto, IncomingCheck::Always] {
            assert!(check(&clone, &sync, strategy, NO_AGENT).is_empty());
            assert!(!log.exists(), "{strategy:?} does not lift the barrier");
        }

        sync.mark_authenticated(clone.path(), "origin");
        check(&clone, &sync, IncomingCheck::Always, NO_AGENT);
        assert!(log.exists());
    }

    #[test]
    fn authentication_belongs_to_that_repository_and_that_remote() {
        let Remote { bare, clone } = remote();
        advance(&bare);
        let sync = BranchSync::default();
        sync.mark_authenticated(Path::new("/another"), "origin");
        sync.mark_authenticated(clone.path(), "another");

        assert!(check(&clone, &sync, IncomingCheck::Always, NO_AGENT).is_empty());
    }

    #[test]
    fn a_successful_ls_remote_counts_as_authentication() {
        let Remote { bare, clone } = remote();
        advance(&bare);
        let sync = BranchSync::default();
        let refs = read_refs(clone.path()).unwrap();
        let hashes = hashes_of(&refs);
        let tracked = tracked_by_remote(&refs);
        let mut found = ToFetch::new();

        runner()
            .quiet(clone.path(), LIMIT, |ctx| {
                check_remote(
                    ctx,
                    &sync,
                    "origin",
                    &tracked["origin"],
                    &hashes,
                    &mut found,
                );
                Ok(())
            })
            .unwrap();

        assert!(found.contains_key("main"));
        assert!(sync.is_authenticated(clone.path(), "origin"));
    }

    #[test]
    fn a_failing_ls_remote_does_not_count_as_authentication() {
        let Remote { clone, .. } = remote();
        clone.git(&["remote", "set-url", "origin", "file:///does/not/exist"]);
        let sync = BranchSync::default();
        let refs = read_refs(clone.path()).unwrap();
        let hashes = hashes_of(&refs);
        let tracked = tracked_by_remote(&refs);

        runner()
            .quiet(clone.path(), LIMIT, |ctx| {
                let mut found = ToFetch::new();
                check_remote(
                    ctx,
                    &sync,
                    "origin",
                    &tracked["origin"],
                    &hashes,
                    &mut found,
                );
                Ok(())
            })
            .unwrap();

        assert!(!sync.is_authenticated(clone.path(), "origin"));
    }

    #[test]
    fn a_hung_remote_does_not_take_the_others_results_away() {
        let Remote { bare, clone } = remote();
        let trace = TempDir::new().unwrap();
        let script = trace.path().join("ssh");
        fs::write(&script, "#!/bin/sh\nexec sleep 30\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        clone.git(&["config", "core.sshCommand", script.to_str().unwrap()]);
        // In alphabetical order: a live one before and another after the hung one.
        clone.git(&["remote", "add", "a", &url(&bare)]);
        clone.git(&["remote", "add", "slow", "ssh://me@127.0.0.1/r.git"]);
        clone.git(&["remote", "add", "z", &url(&bare)]);
        for (branch, remote) in [("first", "a"), ("hung", "slow"), ("last", "z")] {
            let tracking = format!("refs/remotes/{remote}/main");
            clone.git(&["update-ref", &tracking, "HEAD"]);
            clone.git(&["branch", "-q", "--track", branch, &format!("{remote}/main")]);
        }
        advance(&bare);
        let sync = authenticated(&clone);
        // Plenty for the live remotes with the whole suite in parallel: the hung one never finishes.
        let limit = Duration::from_secs(4);
        let started = Instant::now();

        let found = check_remotes_with(
            &runner_with_grace(),
            clone.path(),
            IncomingCheck::Always,
            &sync,
            limit,
            None,
        )
        .unwrap();

        assert!(found.contains_key("first"), "{found:?}");
        assert!(found.contains_key("last"), "{found:?}");
        assert!(!found.contains_key("hung"), "{found:?}");
        assert!(started.elapsed() < Duration::from_secs(20));
    }

    #[test]
    fn with_never_there_are_no_incoming_but_there_are_outgoing() {
        let Remote { bare, clone } = remote();
        clone.commit("b.txt", b"2", "local");
        advance(&bare);
        clone.git(&["fetch", "-q", "origin"]);

        let sync = incoming_outgoing(clone.path(), IncomingCheck::Never, &ToFetch::new()).unwrap();

        assert_eq!(
            sync["main"],
            Sync {
                incoming: false,
                outgoing: true
            }
        );
    }

    #[test]
    fn without_network_the_list_does_not_break() {
        let Remote { bare, clone } = remote();
        clone.commit("b.txt", b"2", "local");
        let missing = format!("file://{}/does-not-exist", path(&bare));
        clone.git(&["remote", "set-url", "origin", &missing]);

        let to_fetch = checked(&clone, IncomingCheck::Always, NO_AGENT);

        assert!(to_fetch.is_empty());
        assert_eq!(list(clone.path()).unwrap().remote, ["origin/main"]);
        let sync = incoming_outgoing(clone.path(), IncomingCheck::Auto, &to_fetch).unwrap();
        assert!(sync["main"].outgoing);
    }

    #[test]
    fn a_down_remote_does_not_take_another_ones_results_away() {
        let Remote { bare, clone } = remote();
        let second = TempRepo::new();
        second.commit("z.txt", b"z", "another");
        clone.git(&["remote", "add", "down", "file:///does/not/exist"]);
        clone.git(&["remote", "add", "alive", &url(&bare)]);
        clone.git(&["fetch", "-q", "alive"]);
        clone.git(&["branch", "-q", "--track", "side", "alive/main"]);
        clone.git(&["config", "branch.main.remote", "down"]);
        clone.git(&["update-ref", "refs/remotes/down/main", "HEAD"]);
        advance(&bare);

        let to_fetch = checked(&clone, IncomingCheck::Always, NO_AGENT);

        assert!(to_fetch.contains_key("side"), "{to_fetch:?}");
        assert!(!to_fetch.contains_key("main"), "{to_fetch:?}");
    }

    #[test]
    fn default_and_excluded_favorites() {
        let dir = TempDir::new().unwrap();

        let defaults = favorites(dir.path());
        assert_eq!(defaults.local, ["main", "master"]);
        assert_eq!(defaults.remote, ["origin/main", "origin/master"]);

        let set = toggle_favorite(dir.path(), "main", BranchKind::Local).unwrap();
        assert_eq!(set.local, ["master"]);
        assert_eq!(favorites(dir.path()).local, ["master"], "it is saved");

        let set = toggle_favorite(dir.path(), "main", BranchKind::Local).unwrap();
        assert_eq!(set.local, ["main", "master"]);

        let stored = fs::read_to_string(dir.path().join(FAVORITES_FILE)).unwrap();
        assert!(
            !stored.contains("main"),
            "a default value is not saved: {stored}"
        );
    }

    #[test]
    fn a_favorite_counts_only_in_its_kind() {
        let dir = TempDir::new().unwrap();

        toggle_favorite(dir.path(), "side", BranchKind::Local).unwrap();
        let set = toggle_favorite(dir.path(), "origin/x", BranchKind::Remote).unwrap();

        assert!(set.local.contains(&"side".to_string()));
        assert!(!set.local.contains(&"origin/x".to_string()));
        assert!(set.remote.contains(&"origin/x".to_string()));

        let set = toggle_favorite(dir.path(), "side", BranchKind::Local).unwrap();
        assert!(!set.local.contains(&"side".to_string()));
    }

    #[test]
    fn a_damaged_file_is_set_aside() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(FAVORITES_FILE), b"{ no").unwrap();

        assert_eq!(favorites(dir.path()).local, ["main", "master"]);
        toggle_favorite(dir.path(), "side", BranchKind::Local).unwrap();

        assert!(favorites(dir.path()).local.contains(&"side".to_string()));
        assert!(dir
            .path()
            .join(format!("{FAVORITES_FILE}.corrupt"))
            .exists());
    }

    #[test]
    fn opening_the_repository_launches_the_check_if_visible() {
        let now = Instant::now();
        let mut schedule = Schedule::default();
        assert_eq!(
            schedule.wait(now),
            None,
            "without a repository there is no timer"
        );

        schedule.request();

        assert!(schedule.should_run(true, now));
        assert!(!schedule.should_run(true, now), "only once");
        assert_eq!(schedule.wait(now), Some(CHECK_PERIOD));
    }

    #[test]
    fn every_twenty_minutes() {
        let now = Instant::now();
        let mut schedule = Schedule::default();
        schedule.request();
        schedule.should_run(true, now);
        let later = now + Duration::from_secs(5 * 60);

        assert_eq!(schedule.wait(later), Some(Duration::from_secs(15 * 60)));

        let due = now + CHECK_PERIOD;
        assert_eq!(schedule.wait(due), Some(Duration::ZERO));
        schedule.request();
        assert!(schedule.should_run(true, due));
        assert_eq!(schedule.wait(due), Some(CHECK_PERIOD));
    }

    #[test]
    fn hidden_waits_without_a_timer_and_runs_when_visible() {
        let now = Instant::now();
        let mut schedule = Schedule::default();
        schedule.request();

        assert!(!schedule.should_run(false, now));
        assert_eq!(schedule.wait(now), None, "no wake-up while it is not visible");

        assert!(schedule.should_run(true, now + CHECK_PERIOD));
    }

    #[test]
    fn the_thread_runs_when_asked_and_what_was_hidden_when_visible_again() {
        let sync = BranchSync::default();
        let shown = Arc::new(AtomicBool::new(false));
        let (done, finished) = mpsc::channel();
        let flag = shown.clone();
        sync.start(
            move || flag.load(Ordering::SeqCst),
            move || done.send(()).unwrap(),
        );

        sync.request();
        assert!(finished.recv_timeout(Duration::from_millis(200)).is_err());

        shown.store(true, Ordering::SeqCst);
        sync.visible();
        assert!(finished.recv_timeout(Duration::from_secs(5)).is_ok());
    }

    #[test]
    fn what_was_seen_belongs_to_the_repository_that_saw_it() {
        let sync = BranchSync::default();
        let found = ToFetch::from([("main".to_string(), "abc".to_string())]);

        sync.store(Path::new("/one"), found.clone());

        assert_eq!(sync.seen(Path::new("/one")), found);
        assert!(sync.seen(Path::new("/two")).is_empty());
    }

    #[test]
    fn the_options_come_back_as_they_were_saved() {
        let dir = TempDir::new().unwrap();
        assert_eq!(read_options(dir.path()), serde_json::json!({}));

        let options = serde_json::json!({ "grouping": false });
        write_options(dir.path(), &options).unwrap();

        assert_eq!(read_options(dir.path()), options);
    }

    #[test]
    fn the_reset_mode_belongs_to_each_repository() {
        let (one, other) = (TempDir::new().unwrap(), TempDir::new().unwrap());
        assert_eq!(read_reset_mode(one.path()), None);

        write_reset_mode(one.path(), ResetMode::Keep).unwrap();
        write_reset_mode(other.path(), ResetMode::Soft).unwrap();

        assert_eq!(read_reset_mode(one.path()), Some(ResetMode::Keep));
        assert_eq!(read_reset_mode(other.path()), Some(ResetMode::Soft));
        write_reset_mode(one.path(), ResetMode::Hard).unwrap();
        assert_eq!(read_reset_mode(one.path()), Some(ResetMode::Hard));
        assert_eq!(read_reset_mode(other.path()), Some(ResetMode::Soft));
    }

    #[test]
    fn a_damaged_or_unknown_reset_mode_is_none() {
        let dir = TempDir::new().unwrap();
        for text in ["{ broken", "\"odd\"", "3", ""] {
            fs::write(dir.path().join("reset-mode.json"), text).unwrap();
            assert_eq!(read_reset_mode(dir.path()), None, "{text}");
        }
    }
}
