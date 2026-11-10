// `Vcs.Push`, with the steps of `GitPushOperation` and `GitPushSupport`. It talks to a remote without
// touching the index, except when updating after a rejection, which is an Update with the same
// barriers. Forcing destroys other people's commits: it is only forced if `PushSpec.force` asks for
// it, and never to a protected branch.
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::branch_info::{self, BranchSync};
pub use crate::branches::LostCommit as CommitSummary;
use crate::branches::{self, Repo, COMMIT_FORMAT};
use crate::console;
use crate::diff;
use crate::fetch;
use crate::git::{self, GitError};
use crate::preserve::RestoreOutcome;
use crate::pull;
use crate::settings::{self, RepoSettings, UpdateMethod};
use crate::storage;
use crate::task::{TaskContext, TaskKind};
use crate::update::{self, Outcome, UpdateOptions, UpdateResult};

const HEAD: &str = "HEAD";
const HEADS: &str = "refs/heads/";
const REMOTES: &str = "refs/remotes/";
const TAGS: &str = "refs/tags/";
const REFS: &str = "refs/";
const ORIGIN: &str = "origin";
const DETACHED: &str = "detached";
const TAGS_FILE: &str = "push-tags.json";
// `GitOutgoingCommitsProvider`.
const NEW_BRANCH_LIMIT: &str = "--max-count=1000";
// `GitPushNativeResult.isNonFFUpdate` y `isStaleInfo`.
const NO_FF_REASONS: [&str; 3] = ["non-fast-forward", "fetch first", "failed to lock"];
const STALE_INFO: &str = "stale info";
const NEW_REF: char = '*';
const FORCED: char = '+';
const LEASE: &str = "--force-with-lease=";
// `GitPushOperation.MAX_PUSH_ATTEMPTS`.
const MAX_PUSH_ATTEMPTS: u32 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PushTags {
    All,
    CurrentBranch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ForceMode {
    Lease,
    Plain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushTarget {
    pub remote: Option<String>,
    pub branch: String,
    pub is_new: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushSpec {
    pub source: String,
    pub remote: String,
    pub target: String,
    pub tags: Option<PushTags>,
    pub force: Option<ForceMode>,
    pub no_verify: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "reason", rename_all = "camelCase")]
pub enum PushStatus {
    Success,
    NewBranch,
    UpToDate,
    Forced,
    RejectedNoFf,
    RejectedStale,
    RejectedOther(String),
    Error(String),
}

// A rejection that is resolved by updating. `merges`: there are unpushed merges, and rebasing them
// causes problems (`GitRebaseOverMergeProblem`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Updatable {
    pub branch: String,
    pub merges: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushResult {
    pub status: PushStatus,
    pub pushed: u32,
    pub source: String,
    pub destination: String,
    pub tags: Vec<String>,
    pub remote: String,
    pub updatable: Option<Updatable>,
}

// `update`: the last Update, with what was received in all of them. `None`: it was never launched.
// `attempts` counts the pushes made, to keep the count if something has to be asked and it comes back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatedPush {
    pub push: PushResult,
    pub update: Option<UpdateResult>,
    pub attempts: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PorcelainLine {
    pub flag: char,
    pub from: String,
    pub to: String,
    pub summary: String,
    pub reason: Option<String>,
    pub range: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushInfo {
    pub repository: String,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub remotes: Vec<String>,
    pub remote_branches: Vec<String>,
    pub target: PushTarget,
    pub pre_push_hook: bool,
    pub protected_branches: Vec<String>,
    pub force_push_targets: Vec<String>,
}

// What is in the refspec's `<src>`: a whole branch, or a commit (`Push All up to Here`, detached
// HEAD), which `GitPushSource` calls `OnRevision` and `DetachedHead`.
enum Source {
    Branch(String),
    Revision(String),
}

impl Source {
    fn revision(&self) -> String {
        match self {
            Self::Branch(name) => format!("{HEADS}{name}"),
            Self::Revision(hash) => hash.clone(),
        }
    }
}

// `pushTags` of `GitVcsOptions`, per repository. `None` is the unchecked box.
pub fn read_tags(dir: &Path) -> Option<PushTags> {
    let bytes = fs::read(dir.join(TAGS_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok().flatten()
}

pub fn write_tags(dir: &Path, tags: Option<PushTags>) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let json = serde_json::to_vec(&tags).map_err(io::Error::other)?;
    storage::write_atomic(&dir.join(TAGS_FILE), &json)
}

pub fn task_kind(spec: &PushSpec) -> TaskKind {
    TaskKind::Remote(spec.remote.clone())
}

fn remotes(path: &Path) -> Result<Vec<String>, GitError> {
    let output = git::run(path, &["remote"])?;
    Ok(String::from_utf8_lossy(&output)
        .lines()
        .map(str::to_string)
        .collect())
}

// `GitUtil.getDefaultOrFirstRemote`.
fn default_remote(remotes: &[String]) -> Option<&str> {
    remotes
        .iter()
        .find(|remote| *remote == ORIGIN)
        .or(remotes.first())
        .map(String::as_str)
}

fn ref_exists(path: &Path, full: &str) -> Result<bool, GitError> {
    match git::run(path, &["show-ref", "--verify", "-q", full]) {
        Ok(_) => Ok(true),
        Err(GitError::Failed { .. }) => Ok(false),
        Err(error) => Err(error),
    }
}

fn remote_branch_exists(path: &Path, remote: &str, branch: &str) -> Result<bool, GitError> {
    ref_exists(path, &format!("{REMOTES}{remote}/{branch}"))
}

// `GitConfig.convertBranchConfig`: what is configured only counts if the remote and its branch
// exist. Tracking a local branch (`.`), a pruned branch or a deleted remote is a stale record.
fn known_upstream(path: &Path, branch: &str) -> Result<Option<(String, String)>, GitError> {
    let Some((remote, name)) = update::tracked_branch(path, branch)? else {
        return Ok(None);
    };
    let known = remotes(path)?.contains(&remote) && remote_branch_exists(path, &remote, &name)?;
    Ok(known.then_some((remote, name)))
}

// `GitPushTarget.parse`: a new one is one that is not known on the remote.
fn known_or_new(path: &Path, remote: &str, branch: &str) -> Result<PushTarget, GitError> {
    Ok(PushTarget {
        remote: Some(remote.to_string()),
        branch: branch.to_string(),
        is_new: !remote_branch_exists(path, remote, branch)?,
    })
}

// `GitBranchUtil.stripRefsPrefix`.
fn strip_refs(name: &str) -> &str {
    [HEADS, REMOTES, TAGS]
        .iter()
        .find_map(|prefix| name.strip_prefix(prefix))
        .unwrap_or(name)
}

// `GitPushSpecParser.getTarget`.
fn spec_target(spec: &str, branch: &str) -> Option<String> {
    let (source, target) = spec.split_once(':')?;
    let (source, target) = (source.trim(), target.trim());
    if target.is_empty() || target.contains(':') {
        return None;
    }
    let source = source.strip_prefix('+').unwrap_or(source);
    let star_at_end = |text: &str| text.find('*').map(|star| star + 1 == text.len());
    if !matches!(
        (star_at_end(source), star_at_end(target)),
        (None, None) | (Some(true), Some(true))
    ) {
        return None;
    }
    let source = strip_refs(source);
    if source == HEAD || source == branch {
        return Some(target.to_string());
    }
    let prefix = source.strip_suffix('*')?;
    let rest = branch.strip_prefix(prefix)?;
    Some(target.replace('*', rest))
}

// `GitPushTarget.getFromPushSpec`. What does not go to a known remote branch is a special reference,
// like `refs/for/main`, and is always new.
fn target_from_push_spec(
    path: &Path,
    remote: &str,
    branch: &str,
) -> Result<Option<PushTarget>, GitError> {
    let key = format!("remote.{remote}.push");
    let specs = match git::run(path, &["config", "--get-all", &key]) {
        Ok(specs) => specs,
        Err(GitError::Failed { .. }) => return Ok(None),
        Err(error) => return Err(error),
    };
    let specs = String::from_utf8_lossy(&specs);
    let Some(target) = specs.lines().find_map(|spec| spec_target(spec, branch)) else {
        return Ok(None);
    };
    let known = format!("{REMOTES}{remote}/");
    Ok(Some(match target.strip_prefix(&known) {
        Some(name) => known_or_new(path, remote, name)?,
        None => PushTarget {
            remote: Some(remote.to_string()),
            branch: target,
            is_new: true,
        },
    }))
}

// `UniqueNameGenerator.generateUniqueName("detached", …)` among that remote's branches.
fn unique_detached(path: &Path, remote: &str) -> Result<String, GitError> {
    let mut name = DETACHED.to_string();
    let mut number = 2;
    while remote_branch_exists(path, remote, &name)? {
        name = format!("{DETACHED}{number}");
        number += 1;
    }
    Ok(name)
}

// `GitPushSupport.getDefaultTarget`. Without `branch`, detached HEAD. Without remotes there is no
// target, and `remote` is left empty: the dialog offers `Define remote`.
pub fn target(path: &Path, branch: Option<&str>) -> Result<PushTarget, GitError> {
    let remotes = remotes(path)?;
    let default = default_remote(&remotes);
    let tracked = match branch {
        Some(branch) => known_upstream(path, branch)?,
        None => None,
    };
    if let Some(branch) = branch {
        let remote = tracked.as_ref().map(|(remote, _)| remote.as_str());
        if let Some(remote) = remote.or(default) {
            if let Some(found) = target_from_push_spec(path, remote, branch)? {
                return Ok(found);
            }
        }
    }
    if let Some((remote, branch)) = tracked {
        return Ok(PushTarget {
            remote: Some(remote),
            branch,
            is_new: false,
        });
    }
    let Some(remote) = default else {
        return Ok(PushTarget {
            remote: None,
            branch: branch.unwrap_or_default().to_string(),
            is_new: false,
        });
    };
    match branch {
        Some(branch) => known_or_new(path, remote, branch),
        None => known_or_new(path, remote, &unique_detached(path, remote)?),
    }
}

// `GitSharedSettings.isBranchProtected`. Java compares the whole string, also in each alternative:
// hence the group. A pattern valid only inside the group, like `a)|(b`, is not valid.
fn protection(pattern: &str) -> Option<Regex> {
    compile(pattern).ok()
}

fn compile(pattern: &str) -> Result<Regex, regex::Error> {
    Regex::new(pattern)?;
    Regex::new(&format!("^(?:{pattern})$"))
}

// While the `;`-separated field is typed: the first pattern that does not compile, with the cause the
// `regex` crate gives (the last `error:` line of its message).
pub fn first_pattern_error(text: &str) -> Option<String> {
    text.split(';')
        .map(str::trim)
        .filter(|pattern| !pattern.is_empty())
        .find_map(|pattern| {
            let error = compile(pattern).err()?;
            let message = error.to_string();
            let cause = message
                .lines()
                .rev()
                .find_map(|line| line.strip_prefix("error: "))
                .unwrap_or(&message);
            Some(format!(
                "Skipped invalid protected branch pattern '{pattern}': {cause}"
            ))
        })
}

// `branch`: the name without the remote.
pub fn is_protected(patterns: &[String], branch: &str) -> bool {
    patterns
        .iter()
        .filter_map(|pattern| protection(pattern))
        .any(|regex| regex.is_match(branch))
}

pub fn invalid_patterns(patterns: &[String]) -> Vec<String> {
    let invalid = patterns
        .iter()
        .filter(|pattern| protection(pattern).is_none());
    invalid.cloned().collect()
}

// `GitRepositoryReader.readHooksInfo`, with the path git gives: it looks at `core.hooksPath` and the worktrees.
pub fn has_pre_push_hook(path: &Path) -> bool {
    let args = ["rev-parse", "--git-path", "hooks/pre-push"];
    let Ok(output) = git::run(path, &args) else {
        return false;
    };
    let output = String::from_utf8_lossy(&output);
    let hook = path.join(output.trim_end_matches('\n'));
    fs::metadata(hook).is_ok_and(|hook| hook.is_file() && hook.permissions().mode() & 0o111 != 0)
}

// What the dialog shows. Without `branch`, the current one. Empty `head`: `Empty repository`.
pub fn info(
    path: &Path,
    repo_settings: RepoSettings,
    branch: Option<&str>,
) -> Result<PushInfo, GitError> {
    if let Some(branch) = branch {
        branches::check_argument(branch, "The branch")?;
    }
    let list = branch_info::list(path)?;
    let branch = branch.map(str::to_string).or(list.current);
    let head = git::run(path, &["rev-parse", "--verify", "-q", HEAD]).ok();
    Ok(PushInfo {
        repository: path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
        head: head.map(|head| String::from_utf8_lossy(&head).trim().to_string()),
        remotes: remotes(path)?,
        remote_branches: list.remote,
        target: target(path, branch.as_deref())?,
        pre_push_hook: has_pre_push_hook(path),
        protected_branches: repo_settings.protected_branches,
        force_push_targets: repo_settings.force_push_targets,
        branch,
    })
}

// `GitPushUpToCommitAction`: the current branch if it contains it, otherwise the first local one that has it.
pub fn branch_of_commit(path: &Path, hash: &str) -> Result<Option<String>, GitError> {
    diff::check_hash(hash)?;
    if let Some(current) = branches::current_name(path)? {
        if git::run(path, &["merge-base", "--is-ancestor", hash, HEAD]).is_ok() {
            return Ok(Some(current));
        }
    }
    let args = [
        "for-each-ref",
        "--format=%(refname)",
        "--contains",
        hash,
        HEADS,
    ];
    let output = git::run(path, &args)?;
    let output = String::from_utf8_lossy(&output);
    Ok(output
        .lines()
        .find_map(|name| name.strip_prefix(HEADS))
        .map(str::to_string))
}

// `GitPushNativeResultParser`: `<flag>\t<from>:<to>\t<summary> (<reason>)`.
pub fn parse_porcelain(stdout: &str) -> Vec<PorcelainLine> {
    static LINE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^.*([ +\-*!=])\t(\S+):(\S+)\t([^(]+)(?:\((.+)\))?.*$").expect("fixed pattern")
    });
    static RANGE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^[0-9a-f]+\.{2,3}[0-9a-f]+$").expect("fixed pattern"));
    stdout
        .lines()
        .filter_map(|line| LINE.captures(line))
        .map(|found| {
            let summary = found[4].trim().to_string();
            PorcelainLine {
                flag: found[1].chars().next().expect("one letter"),
                from: found[2].to_string(),
                to: found[3].to_string(),
                range: RANGE.is_match(&summary).then(|| summary.clone()),
                summary,
                reason: found.get(5).map(|reason| reason.as_str().to_string()),
            }
        })
        .collect()
}

// `GitPushRepoResult.convertType`.
fn status_of(line: &PorcelainLine) -> PushStatus {
    match line.flag {
        ' ' => PushStatus::Success,
        FORCED => PushStatus::Forced,
        NEW_REF => PushStatus::NewBranch,
        '=' => PushStatus::UpToDate,
        '!' => {
            let reason = line.reason.as_deref().unwrap_or(&line.summary);
            let lower = reason.to_lowercase();
            if NO_FF_REASONS.iter().any(|known| lower.contains(known)) {
                PushStatus::RejectedNoFf
            } else if lower.contains(STALE_INFO) {
                PushStatus::RejectedStale
            } else {
                PushStatus::RejectedOther(reason.to_string())
            }
        }
        _ => PushStatus::Error(line.summary.clone()),
    }
}

// A commit is not confused with a branch of the same name: what rules is that the branch exists.
fn check_source(ctx: &TaskContext, source: &str) -> Result<Source, GitError> {
    let branch = format!("{HEADS}{source}");
    if diff::check_hash(source).is_ok() && !ref_exists(ctx.repo(), &branch)? {
        return Ok(Source::Revision(source.to_string()));
    }
    pull::check_branch(ctx, source)?;
    Ok(Source::Branch(source.to_string()))
}

fn check_target(ctx: &TaskContext, remote: &str, branch: &str) -> Result<PushTarget, GitError> {
    pull::check_remote(ctx, remote)?;
    pull::check_branch(ctx, branch)?;
    known_or_new(ctx.repo(), remote, branch)
}

fn outgoing_args(source: &str, remote: &str, branch: &str, is_new: bool) -> Vec<String> {
    let mut args: Vec<String> = ["-c", "log.showSignature=false", "log", COMMIT_FORMAT]
        .map(str::to_string)
        .to_vec();
    if is_new {
        args.extend([
            source.to_string(),
            "--not".to_string(),
            format!("--remotes={remote}"),
            NEW_BRANCH_LIMIT.to_string(),
        ]);
    } else {
        args.push(format!("{REMOTES}{remote}/{branch}..{source}"));
    }
    args.push("--".to_string());
    args
}

// `GitOutgoingCommitsProvider`.
pub fn outgoing(
    ctx: &TaskContext,
    source: &str,
    remote: &str,
    target: &str,
) -> Result<Vec<CommitSummary>, GitError> {
    let source = check_source(ctx, source)?;
    let target = check_target(ctx, remote, target)?;
    let args = outgoing_args(&source.revision(), remote, &target.branch, target.is_new);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    Ok(branches::commits_in(&ctx.git(&args)?.stdout))
}

// The target as a branch: what the patterns and the tracking references understand.
fn branch_name(target: &str) -> &str {
    target.strip_prefix(HEADS).unwrap_or(target)
}

// `GitPushSupport.isForcePushAllowed`. Before anything else and without git: the dialog disables the
// button, but it is here that it cannot get through. What cannot be read counts as forbidden.
fn check_force(repo: &Repo, spec: &PushSpec) -> Result<(), GitError> {
    if spec.force.is_none() {
        return Ok(());
    }
    let patterns = settings::try_effective_protected(repo.data)
        .map_err(|error| GitError::Io(error.to_string()))?;
    let branch = branch_name(&spec.target);
    if is_protected(&patterns, branch) {
        return Err(GitError::Io(format!(
            "Force push to {branch} is prohibited"
        )));
    }
    Ok(())
}

// `repository.getBranches().getHash(targetBranch)`, when pushing and not when opening the dialog.
// Empty if unknown: the remote must not have that branch.
fn tracking_hash(ctx: &TaskContext, remote: &str, branch: &str) -> Result<String, GitError> {
    let full = format!("{REMOTES}{remote}/{branch}");
    match ctx.git(&["rev-parse", "--verify", "-q", &full]) {
        Ok(output) => Ok(output.stdout.trim().to_string()),
        Err(GitError::Failed { .. }) => Ok(String::new()),
        Err(error) => Err(error),
    }
}

// `GitPushOperation.doPush` and `createPushSpec`: `--set-upstream` is automatic, and with it, or with
// a commit, git needs to know the target is a branch. The order is that of `GitImpl.doPush`.
fn push_args(repo: &Repo, spec: &PushSpec) -> Result<Vec<String>, GitError> {
    let ctx = repo.ctx;
    check_force(repo, spec)?;
    let name = branch_name(&spec.target);
    let source = check_source(ctx, &spec.source)?;
    let target = check_target(ctx, &spec.remote, name)?;
    let tracked = match &source {
        Source::Branch(name) => known_upstream(ctx.repo(), name)?.is_some(),
        Source::Revision(_) => true,
    };
    let set_upstream = target.is_new && !tracked;
    // When forcing too: with other names git reaches the same branch (`heads/main` is `main`), and
    // what was checked must be what gets overwritten.
    let forced = spec.force.is_some();
    let full_name = set_upstream || forced || matches!(source, Source::Revision(_));
    let destination = if full_name && !spec.target.starts_with(REFS) {
        format!("{HEADS}{}", spec.target)
    } else {
        spec.target.clone()
    };
    // Only this refspec, and not `--force`: that would also force whatever `--tags` pushes. Another
    // deviation from IntelliJ.
    let plus = if spec.force == Some(ForceMode::Plain) {
        "+"
    } else {
        ""
    };
    let refspec = format!("{plus}{}:{destination}", source.revision());
    let mut args: Vec<String> = ["push", "--progress", "--porcelain", &spec.remote, &refspec]
        .map(str::to_string)
        .to_vec();
    if set_upstream {
        args.push("--set-upstream".to_string());
    }
    // With the full name: the short one would also reach a tag named like the branch.
    if spec.force == Some(ForceMode::Lease) {
        let hash = tracking_hash(ctx, &spec.remote, name)?;
        args.push(format!("{LEASE}{destination}:{hash}"));
    }
    match spec.tags {
        Some(PushTags::All) => args.push("--tags".to_string()),
        Some(PushTags::CurrentBranch) => args.push("--follow-tags".to_string()),
        None => {}
    }
    if spec.no_verify && has_pre_push_hook(ctx.repo()) {
        args.push("--no-verify".to_string());
    }
    Ok(args)
}

fn result_of(spec: &PushSpec, status: PushStatus) -> PushResult {
    PushResult {
        status,
        pushed: 0,
        source: spec.source.clone(),
        destination: if spec.target.starts_with(REFS) {
            spec.target.clone()
        } else {
            format!("{}/{}", spec.remote, spec.target)
        },
        tags: Vec::new(),
        remote: spec.remote.clone(),
        updatable: None,
    }
}

// `collectNumberOfPushedCommits`. What gets pushed is already pushed: cancelling here changes nothing.
fn pushed_commits(ctx: &TaskContext, line: &PorcelainLine) -> u32 {
    let (' ', Some(range)) = (line.flag, &line.range) else {
        return 0;
    };
    let args = ["rev-list", "--count", range, "--"];
    ctx.git_to_the_end(&args)
        .map_or(0, |output| output.stdout.trim().parse().unwrap_or(0))
}

// `getPushedBranchOrCommit`: with `--tags` the tags come out too, in any order.
fn pushed_line(lines: &[PorcelainLine]) -> Option<&PorcelainLine> {
    lines
        .iter()
        .find(|line| line.from.starts_with(HEADS) || diff::check_hash(&line.from).is_ok())
}

// A failure is just another result; cancelling is only done with `Err`.
pub fn push(repo: &Repo, sync: &BranchSync, spec: &PushSpec) -> Result<PushResult, GitError> {
    let ctx = repo.ctx;
    let args = match push_args(repo, spec) {
        Ok(args) => args,
        Err(GitError::Cancelled) => return Err(GitError::Cancelled),
        Err(error) => {
            let status = PushStatus::Error(fetch::error_text(error));
            return Ok(result_of(spec, status));
        }
    };
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    // A rejection ends with a git error, but what matters goes through standard output.
    let (stdout, stderr) = match ctx.git_loud(&args) {
        Ok(output) => {
            sync.mark_authenticated(ctx.repo(), &spec.remote);
            (output.stdout, output.stderr)
        }
        Err(GitError::Failed { stdout, stderr, .. }) => (stdout, stderr),
        Err(GitError::Cancelled) => return Err(GitError::Cancelled),
        Err(error) => {
            let status = PushStatus::Error(fetch::error_text(error));
            return Ok(result_of(spec, status));
        }
    };
    let lines = parse_porcelain(&stdout);
    let Some(line) = pushed_line(&lines) else {
        let text = console::hide_credentials(stderr.trim());
        return Ok(result_of(spec, PushStatus::Error(text)));
    };
    let status = status_of(line);
    Ok(PushResult {
        pushed: pushed_commits(ctx, line),
        tags: lines
            .iter()
            .filter(|other| !std::ptr::eq(*other, line))
            .filter(|other| matches!(other.flag, NEW_REF | FORCED))
            .map(|other| strip_refs(&other.from).to_string())
            .collect(),
        updatable: if status == PushStatus::RejectedNoFf {
            updatable(ctx, spec)
        } else {
            None
        },
        ..result_of(spec, status)
    })
}

// `pushingNotCurrentBranch` and `pushingToNotTrackedBranch`, and without forcing: whoever forces does
// not want what is outside. The branch that can be updated.
fn current_tracked(path: &Path, spec: &PushSpec) -> Result<Option<String>, GitError> {
    let Some(current) = branches::current_name(path)?.filter(|_| spec.force.is_none()) else {
        return Ok(None);
    };
    let target = (spec.remote.clone(), branch_name(&spec.target).to_string());
    let tracked = known_upstream(path, &current)?;
    Ok((spec.source == current && tracked == Some(target)).then_some(current))
}

// Knowing about the merges is an aid for the dialog: if it cannot be known, there are none.
fn updatable(ctx: &TaskContext, spec: &PushSpec) -> Option<Updatable> {
    let branch = current_tracked(ctx.repo(), spec).ok().flatten()?;
    let base = format!("{REMOTES}{}/{}", spec.remote, branch_name(&spec.target));
    let merges = update::has_resolved_merges(ctx, &base, &format!("{HEADS}{branch}"));
    Some(Updatable {
        branch,
        merges: merges.unwrap_or(false),
    })
}

// There is something to report about what was saved: the `Restore Changes` card or the stage notice.
fn has_saved_news(update: &UpdateResult) -> bool {
    let done = matches!(
        update.restore,
        Some(RestoreOutcome::Restored | RestoreOutcome::Gone)
    );
    update.saved.is_some() && !done
}

// `joinUpdatedRanges`: from the start of the first to the end of the last. Of what was saved, what
// still has to be reported remains, and failing that the latest that is known.
fn joined(before: Option<UpdateResult>, update: UpdateResult) -> UpdateResult {
    let Some(before) = before else {
        return update;
    };
    let keep_before =
        update.saved.is_none() || (has_saved_news(&before) && !has_saved_news(&update));
    let (saved, restore) = if keep_before {
        (before.saved, before.restore)
    } else {
        (update.saved.clone(), update.restore.clone())
    };
    let range = match (before.range, update.range.clone()) {
        (Some((start, _)), Some((_, end))) => Some((start, end)),
        (range, other) => other.or(range),
    };
    UpdateResult {
        files: before.files + update.files,
        commits: before.commits + update.commits,
        range,
        saved,
        restore,
        ..update
    }
}

// The loop of `GitPushOperation.execute`, without the first push: that one was already done, and in
// between it was asked what to do. `update` receives whether it is the first, and `None` says it can
// no longer be updated. A push that is cancelled does not take away what the Update did: what happened
// is returned.
fn retrying(
    rejected: PushResult,
    attempts: u32,
    mut update: impl FnMut(bool) -> Option<UpdateResult>,
    mut push: impl FnMut() -> Result<PushResult, GitError>,
) -> UpdatedPush {
    let mut result = UpdatedPush {
        push: rejected,
        update: None,
        attempts: attempts.max(1),
    };
    while result.attempts < MAX_PUSH_ATTEMPTS && result.push.status == PushStatus::RejectedNoFf {
        let Some(updated) = update(result.update.is_none()) else {
            break;
        };
        // With changes still in the stash, going on would cover the card that returns them.
        let done = matches!(updated.outcome, Outcome::Success | Outcome::NothingToUpdate)
            && !updated.still_saved();
        result.update = Some(joined(result.update.take(), updated));
        if !done {
            break;
        }
        match push() {
            Ok(pushed) => result.push = pushed,
            Err(GitError::Cancelled) => break,
            Err(error) => {
                result.push.status = PushStatus::Error(fetch::error_text(error));
                break;
            }
        }
        result.attempts += 1;
    }
    result
}

// `update` carries Update's barriers: its lock only from saving to restoring. It is checked every
// time whether it is still the current branch, because it may have changed in between. The push, in its turn.
pub fn update_and_push(
    repo: &Repo,
    sync: &BranchSync,
    spec: &PushSpec,
    method: UpdateMethod,
    options: UpdateOptions,
    attempts: u32,
    flush: impl Fn() -> Result<(), GitError>,
) -> UpdatedPush {
    let ctx = repo.ctx;
    retrying(
        result_of(spec, PushStatus::RejectedNoFf),
        attempts,
        |first| {
            current_tracked(repo.path, spec).ok().flatten()?;
            let options = UpdateOptions {
                fetched: first && options.fetched,
                ..options.clone()
            };
            Some(update::update_project(repo, sync, method, options, &flush))
        },
        || {
            ctx.on_remote(&spec.remote, || push(repo, sync, spec))
                .ok_or(GitError::Cancelled)?
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::RepoLocks;
    use crate::console::Console;
    use crate::hosting::protection::testing::{rules, GithubRemote};
    use crate::preserve::Saved;
    use crate::task::Runner;
    use crate::test_support::{in_task, TempRepo};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{mpsc, Arc};
    use std::time::Duration;
    use tempfile::TempDir;

    const WAIT: Duration = Duration::from_secs(60);

    struct Harness {
        repo: TempRepo,
        origin: TempDir,
        data: TempDir,
        console: Arc<Console>,
        runner: Runner,
        sync: Arc<BranchSync>,
    }

    fn bare() -> TempDir {
        let dir = TempDir::new().unwrap();
        let status = crate::test_support::git_command()
            .args(["init", "-q", "--bare", "-b", "main"])
            .arg(dir.path())
            .status()
            .unwrap();
        assert!(status.success());
        dir
    }

    fn spec(source: &str, target: &str) -> PushSpec {
        PushSpec {
            source: source.to_string(),
            remote: "origin".to_string(),
            target: target.to_string(),
            tags: None,
            force: None,
            no_verify: false,
        }
    }

    impl Harness {
        fn new() -> Self {
            let origin = bare();
            let repo = TempRepo::new();
            repo.commit("a.txt", b"1\n", "one");
            repo.git(&["remote", "add", "origin", origin.path().to_str().unwrap()]);
            repo.git(&["push", "-q", "-u", "origin", "main"]);
            let console = Arc::<Console>::default();
            let runner = Runner::with(
                Arc::<RepoLocks>::default(),
                console.clone(),
                None,
                Duration::from_secs(5),
            );
            Self {
                repo,
                origin,
                data: TempDir::new().unwrap(),
                console,
                runner,
                sync: Arc::default(),
            }
        }

        fn path(&self) -> &Path {
            self.repo.path()
        }

        fn push(&self, spec: PushSpec) -> PushResult {
            let (send, receive) = mpsc::channel();
            let sync = self.sync.clone();
            let path = self.path().to_path_buf();
            let data = self.data.path().to_path_buf();
            self.runner.start(
                self.path(),
                task_kind(&spec),
                "Pushing…".to_string(),
                move |ctx| {
                    let repo = Repo {
                        ctx,
                        path: &path,
                        data: &data,
                    };
                    send.send(push(&repo, &sync, &spec)).unwrap();
                    Ok(())
                },
            );
            receive.recv_timeout(WAIT).unwrap().unwrap()
        }

        fn update_and_push(&self, spec: PushSpec, method: UpdateMethod) -> UpdatedPush {
            let (send, receive) = mpsc::channel();
            let sync = self.sync.clone();
            let path = self.path().to_path_buf();
            let data = self.data.path().to_path_buf();
            self.runner.start(
                self.path(),
                TaskKind::Read,
                "Pushing…".to_string(),
                move |ctx| {
                    let repo = Repo {
                        ctx,
                        path: &path,
                        data: &data,
                    };
                    let options = UpdateOptions::default();
                    let flush = || Ok(());
                    send.send(update_and_push(
                        &repo, &sync, &spec, method, options, 1, flush,
                    ))
                    .unwrap();
                    Ok(())
                },
            );
            receive.recv_timeout(WAIT).unwrap()
        }

        fn protect(&self, patterns: &[&str]) {
            let patterns = patterns.iter().map(|pattern| pattern.to_string()).collect();
            settings::save_protected_branches(self.data.path(), patterns).unwrap();
        }

        // `feature`, already on the remote, with a commit rewritten here.
        fn rewritten_feature(&self) -> String {
            self.repo.git(&["checkout", "-q", "-b", "feature"]);
            self.repo.commit("f.txt", b"f\n", "feature");
            self.repo.git(&["push", "-q", "-u", "origin", "feature"]);
            let old = self.head();
            self.repo
                .git(&["commit", "-q", "--amend", "-m", "rewritten"]);
            old
        }

        // A clone with that branch advances on the remote without it being known here.
        fn someone_else_pushes_to(&self, branch: &str) {
            let colleague = TempRepo::new();
            let url = self.origin.path().to_str().unwrap();
            colleague.git(&["remote", "add", "origin", url]);
            colleague.git(&["fetch", "-q", "origin"]);
            let tracking = format!("origin/{branch}");
            colleague.git(&["checkout", "-q", "-B", branch, &tracking]);
            colleague.commit("theirs.txt", b"s\n", "from outside");
            colleague.git(&["push", "-q", "origin", branch]);
        }

        fn outgoing(&self, source: &str, target: &str) -> Result<Vec<CommitSummary>, GitError> {
            let (source, target) = (source.to_string(), target.to_string());
            in_task(&self.runner, self.path(), move |ctx| {
                outgoing(ctx, &source, "origin", &target)
            })
        }

        fn ran(&self, command: &str) -> bool {
            self.console
                .lines()
                .iter()
                .any(|line| line.command && line.text.ends_with(command))
        }

        fn ran_anything_with(&self, word: &str) -> bool {
            self.console
                .lines()
                .iter()
                .any(|line| line.command && line.text.split(' ').any(|part| part == word))
        }

        fn hook(&self) {
            let hook = self.path().join(".git/hooks/pre-push");
            fs::create_dir_all(hook.parent().unwrap()).unwrap();
            fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        }

        fn remote_hash(&self, reference: &str) -> String {
            let output = crate::test_support::git_command()
                .arg("-C")
                .arg(self.origin.path())
                .args(["rev-parse", "--verify", "-q", reference])
                .output()
                .unwrap();
            String::from_utf8(output.stdout).unwrap().trim().to_string()
        }

        fn head(&self) -> String {
            self.repo.git(&["rev-parse", "HEAD"]).trim().to_string()
        }

        // Another clone pushes a commit that this repository does not have yet.
        fn someone_else_pushes(&self) {
            let colleague = TempRepo::new();
            let url = self.origin.path().to_str().unwrap();
            colleague.git(&["remote", "add", "origin", url]);
            colleague.git(&["fetch", "-q", "origin"]);
            colleague.git(&["checkout", "-q", "-B", "main", "origin/main"]);
            colleague.commit("theirs.txt", b"s\n", "from outside");
            colleague.git(&["push", "-q", "origin", "main"]);
        }
    }

    // The default target.

    #[test]
    fn target_is_the_tracked_one() {
        let harness = Harness::new();
        harness.repo.git(&["remote", "add", "another", "/does/not/matter"]);
        harness.repo.git(&["branch", "-q", "local"]);
        harness
            .repo
            .git(&["config", "branch.local.remote", "origin"]);
        harness
            .repo
            .git(&["config", "branch.local.merge", "refs/heads/main"]);

        let found = target(harness.path(), Some("local")).unwrap();

        assert_eq!(
            found,
            PushTarget {
                remote: Some("origin".to_string()),
                branch: "main".to_string(),
                is_new: false,
            }
        );
    }

    #[test]
    fn without_tracked_uses_origin_and_the_same_name() {
        let harness = Harness::new();
        harness.repo.git(&["remote", "add", "aaa", "/does/not/matter"]);
        harness.repo.git(&["branch", "-q", "new"]);

        let found = target(harness.path(), Some("new")).unwrap();

        assert_eq!(
            found,
            PushTarget {
                remote: Some("origin".to_string()),
                branch: "new".to_string(),
                is_new: true,
            }
        );
    }

    #[test]
    fn without_origin_uses_the_first_remote() {
        let repo = TempRepo::new();
        repo.git(&["remote", "add", "zzz", "/does/not/matter"]);
        repo.git(&["remote", "add", "bbb", "/does/not/matter"]);

        let found = target(repo.path(), Some("main")).unwrap();

        assert_eq!(found.remote.as_deref(), Some("bbb"));
    }

    #[test]
    fn without_tracked_but_with_that_branch_on_the_remote_it_is_not_new() {
        let harness = Harness::new();
        harness.repo.git(&["branch", "-q", "already"]);
        harness.repo.git(&["push", "-q", "origin", "already"]);

        let found = target(harness.path(), Some("already")).unwrap();

        assert!(!found.is_new);
    }

    #[test]
    fn without_remotes_it_asks_to_define_one() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1\n", "one");

        let found = target(repo.path(), Some("main")).unwrap();

        assert_eq!(found.remote, None);
        assert_eq!(found.branch, "main");
    }

    #[test]
    fn detached_head_proposes_detached_without_overwriting_a_branch() {
        let harness = Harness::new();
        assert_eq!(
            target(harness.path(), None).unwrap().branch,
            "detached",
            "with none of that name"
        );

        harness.repo.git(&["branch", "-q", "detached"]);
        harness.repo.git(&["push", "-q", "origin", "detached"]);

        let found = target(harness.path(), None).unwrap();
        assert_eq!(found.branch, "detached2");
        assert!(found.is_new);
    }

    #[test]
    fn the_tracked_of_a_local_branch_is_not_a_remote() {
        let harness = Harness::new();
        harness.repo.git(&["branch", "-q", "base"]);
        harness
            .repo
            .git(&["checkout", "-q", "-b", "child", "--track", "base"]);

        let found = target(harness.path(), Some("child")).unwrap();

        assert_eq!(found.remote.as_deref(), Some("origin"));
        assert_eq!(found.branch, "child");
    }

    // A `gone` branch that tracked `origin/other`, already deleted from the remote and pruned here.
    fn pruned_upstream(harness: &Harness) {
        harness.repo.git(&["branch", "-q", "other"]);
        harness.repo.git(&["push", "-q", "origin", "other"]);
        harness
            .repo
            .git(&["checkout", "-q", "-b", "gone", "--track", "origin/other"]);
        harness
            .repo
            .git(&["update-ref", "-d", "refs/remotes/origin/other"]);
    }

    // An `orphan` branch whose tracked remote is no longer configured.
    fn upstream_of_a_deleted_remote(harness: &Harness) {
        let url = harness.origin.path().to_str().unwrap();
        harness.repo.git(&["remote", "add", "old", url]);
        harness.repo.git(&["fetch", "-q", "old"]);
        harness
            .repo
            .git(&["checkout", "-q", "-b", "orphan", "--track", "old/main"]);
        harness
            .repo
            .git(&["config", "--remove-section", "remote.old"]);
    }

    #[test]
    fn a_pruned_tracked_branch_does_not_count_and_proposes_the_same_name_as_new() {
        let harness = Harness::new();
        pruned_upstream(&harness);

        let found = target(harness.path(), Some("gone")).unwrap();

        assert_eq!(
            found,
            PushTarget {
                remote: Some("origin".to_string()),
                branch: "gone".to_string(),
                is_new: true,
            }
        );
    }

    #[test]
    fn the_tracked_branch_of_a_deleted_remote_does_not_count() {
        let harness = Harness::new();
        upstream_of_a_deleted_remote(&harness);

        let found = target(harness.path(), Some("orphan")).unwrap();

        assert_eq!(found.remote.as_deref(), Some("origin"));
        assert_eq!(found.branch, "orphan");
        assert!(found.is_new);
    }

    #[test]
    fn the_push_refspec_of_the_remote_overrides_the_tracked_one() {
        let harness = Harness::new();
        let cases = [
            ("refs/heads/main:refs/for/main", "refs/for/main"),
            ("+refs/heads/*:refs/for/*", "refs/for/main"),
            ("HEAD:refs/for/all", "refs/for/all"),
        ];
        for (refspec, wanted) in cases {
            harness.repo.git(&["config", "remote.origin.push", refspec]);

            let found = target(harness.path(), Some("main")).unwrap();

            assert_eq!(found.branch, wanted, "{refspec}");
            assert!(found.is_new, "{refspec}");
        }
    }

    #[test]
    fn a_refspec_pointing_to_a_known_remote_branch_is_not_new() {
        let harness = Harness::new();
        harness.repo.git(&[
            "config",
            "remote.origin.push",
            "refs/heads/*:refs/remotes/origin/*",
        ]);

        let found = target(harness.path(), Some("main")).unwrap();

        assert_eq!(found.branch, "main");
        assert!(!found.is_new);
    }

    #[test]
    fn a_refspec_that_does_not_apply_to_it_does_not_count() {
        let harness = Harness::new();
        harness.repo.git(&[
            "config",
            "remote.origin.push",
            "refs/heads/other:refs/for/other",
        ]);

        let found = target(harness.path(), Some("main")).unwrap();

        assert_eq!(found.branch, "main");
        assert!(!found.is_new);
        for spec in [
            "refs/heads/*:refs/for/x",
            "refs/heads/main",
            "a:b:c",
            "main:",
        ] {
            assert_eq!(spec_target(spec, "main"), None, "{spec}");
        }
    }

    // The command.

    #[test]
    fn push_arguments() {
        let harness = Harness::new();
        harness.repo.commit("b.txt", b"b\n", "two");

        let result = harness.push(spec("main", "main"));

        assert!(harness.ran("push --progress --porcelain origin refs/heads/main:main"));
        assert_eq!(result.status, PushStatus::Success);
    }

    #[test]
    fn new_branch_carries_set_upstream_and_full_path() {
        let harness = Harness::new();
        harness.repo.git(&["checkout", "-q", "-b", "x"]);

        let result = harness.push(spec("x", "x"));

        assert!(harness
            .ran("push --progress --porcelain origin refs/heads/x:refs/heads/x --set-upstream"));
        assert_eq!(result.status, PushStatus::NewBranch);
        assert_eq!(
            harness.repo.git(&["config", "branch.x.merge"]).trim(),
            "refs/heads/x"
        );
    }

    #[test]
    fn new_branch_that_already_tracks_another_does_not_change_upstream() {
        let harness = Harness::new();
        harness
            .repo
            .git(&["checkout", "-q", "-b", "x", "origin/main"]);

        let result = harness.push(spec("x", "other"));

        assert_eq!(result.status, PushStatus::NewBranch);
        assert!(harness.ran("push --progress --porcelain origin refs/heads/x:other"));
        assert_eq!(
            harness.repo.git(&["config", "branch.x.merge"]).trim(),
            "refs/heads/main"
        );
    }

    #[test]
    fn new_branch_tracking_a_local_one_carries_set_upstream() {
        let harness = Harness::new();
        harness.repo.git(&["branch", "-q", "base"]);
        harness
            .repo
            .git(&["checkout", "-q", "-b", "child", "--track", "base"]);

        let result = harness.push(spec("child", "child"));

        assert_eq!(result.status, PushStatus::NewBranch);
        assert!(harness.ran("origin refs/heads/child:refs/heads/child --set-upstream"));
        assert_eq!(
            harness.repo.git(&["config", "branch.child.remote"]).trim(),
            "origin"
        );
    }

    #[test]
    fn new_branch_with_a_pruned_tracked_branch_carries_set_upstream() {
        let harness = Harness::new();
        pruned_upstream(&harness);

        let result = harness.push(spec("gone", "gone"));

        assert_eq!(result.status, PushStatus::NewBranch);
        assert!(harness.ran("origin refs/heads/gone:refs/heads/gone --set-upstream"));
        assert_eq!(
            harness.repo.git(&["config", "branch.gone.merge"]).trim(),
            "refs/heads/gone"
        );
    }

    #[test]
    fn new_branch_with_the_tracked_one_on_a_deleted_remote_carries_set_upstream() {
        let harness = Harness::new();
        upstream_of_a_deleted_remote(&harness);

        let result = harness.push(spec("orphan", "orphan"));

        assert_eq!(result.status, PushStatus::NewBranch);
        assert!(harness.ran("origin refs/heads/orphan:refs/heads/orphan --set-upstream"));
        assert_eq!(
            harness
                .repo
                .git(&["config", "branch.orphan.remote"])
                .trim(),
            "origin"
        );
    }

    #[test]
    fn a_special_target_is_left_alone() {
        let harness = Harness::new();
        harness.repo.git(&["checkout", "-q", "-b", "x"]);

        let result = harness.push(spec("x", "refs/for/main"));

        assert!(harness.ran("origin refs/heads/x:refs/for/main --set-upstream"));
        assert_eq!(result.status, PushStatus::NewBranch);
        assert_eq!(result.destination, "refs/for/main");
    }

    #[test]
    fn tags_all_uses_tags() {
        let harness = Harness::new();
        harness.repo.git(&["tag", "loose"]);

        let result = harness.push(PushSpec {
            tags: Some(PushTags::All),
            ..spec("main", "main")
        });

        assert!(harness.ran("origin refs/heads/main:main --tags"));
        assert_eq!(result.status, PushStatus::UpToDate);
        assert_eq!(result.tags, ["loose"]);
    }

    #[test]
    fn tags_current_uses_follow_tags() {
        let harness = Harness::new();
        harness.repo.commit("b.txt", b"b\n", "two");
        harness.repo.git(&["tag", "-a", "-m", "annotated", "v1"]);
        harness.repo.git(&["tag", "light"]);

        let result = harness.push(PushSpec {
            tags: Some(PushTags::CurrentBranch),
            ..spec("main", "main")
        });

        assert!(harness.ran("origin refs/heads/main:main --follow-tags"));
        assert_eq!(result.status, PushStatus::Success);
        assert_eq!(result.tags, ["v1"]);
        assert_eq!(result.remote, "origin");
    }

    #[test]
    fn no_verify_only_if_unchecked_with_hook() {
        let harness = Harness::new();
        harness.repo.commit("b.txt", b"b\n", "two");

        harness.push(PushSpec {
            no_verify: true,
            ..spec("main", "main")
        });
        assert!(
            !harness.ran_anything_with("--no-verify"),
            "without a hook there is nothing to skip"
        );

        harness.repo.commit("c.txt", b"c\n", "three");
        harness.hook();
        assert!(matches!(
            harness.push(spec("main", "main")).status,
            PushStatus::Error(_)
        ));
        assert!(!harness.ran_anything_with("--no-verify"));

        let result = harness.push(PushSpec {
            no_verify: true,
            ..spec("main", "main")
        });
        assert!(harness.ran("origin refs/heads/main:main --no-verify"));
        assert_eq!(result.status, PushStatus::Success);
    }

    #[test]
    fn hook_that_cannot_be_executed_does_not_count() {
        let harness = Harness::new();
        assert!(!has_pre_push_hook(harness.path()));

        harness.hook();
        assert!(has_pre_push_hook(harness.path()));

        let hook = harness.path().join(".git/hooks/pre-push");
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!has_pre_push_hook(harness.path()));
    }

    #[test]
    fn hook_in_core_hookspath() {
        let harness = Harness::new();
        let hooks = TempDir::new().unwrap();
        let hook = hooks.path().join("pre-push");
        fs::write(&hook, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();

        harness
            .repo
            .git(&["config", "core.hooksPath", hooks.path().to_str().unwrap()]);

        assert!(has_pre_push_hook(harness.path()));
    }

    #[test]
    fn push_up_to_commit_uses_hash() {
        let harness = Harness::new();
        let first = harness.repo.commit("b.txt", b"b\n", "two");
        harness.repo.commit("c.txt", b"c\n", "three");

        let result = harness.push(spec(&first, "main"));

        let refspec = format!("origin {first}:refs/heads/main");
        assert!(harness.ran(&refspec));
        assert_eq!(result.status, PushStatus::Success);
        assert_eq!(result.pushed, 1);
        assert_eq!(result.source, first);
        assert_eq!(harness.remote_hash("refs/heads/main"), first);
    }

    #[test]
    fn a_commit_to_a_new_branch_does_not_change_the_upstream() {
        let harness = Harness::new();
        let head = harness.head();

        let result = harness.push(spec(&head, "new"));

        assert_eq!(result.status, PushStatus::NewBranch);
        assert!(!harness.ran_anything_with("--set-upstream"));
    }

    // What cannot reach git.

    #[test]
    fn nothing_from_the_interface_reaches_git_as_an_option_or_forces_a_push() {
        let harness = Harness::new();
        harness.repo.commit("b.txt", b"b\n", "two");
        let url = harness.origin.path().to_str().unwrap().to_string();
        let cases = [
            ("--upload-pack=touch /tmp/not-created", "main", "main"),
            ("ext::sh -c touch% /tmp/not-created", "main", "main"),
            (url.as_str(), "main", "main"),
            ("", "main", "main"),
            ("another", "main", "main"),
            ("origin", "--force", "main"),
            ("origin", "-f", "main"),
            ("origin", "+main", "main"),
            ("origin", "", "main"),
            ("origin", "main:main", "main"),
            ("origin", "HEAD", "main"),
            ("origin", "main", "--force"),
            ("origin", "main", "-f"),
            ("origin", "main", "+main"),
            ("origin", "main", ""),
            ("origin", "main", "a b"),
            ("origin", "main", "a:b"),
            ("origin", "main", "*"),
            ("origin", "main", "HEAD"),
        ];
        for (remote, source, target) in cases {
            let result = harness.push(PushSpec {
                remote: remote.to_string(),
                ..spec(source, target)
            });

            assert!(
                matches!(result.status, PushStatus::Error(_)),
                "{remote:?} {source:?} {target:?}: {:?}",
                result.status
            );
        }
        assert!(!harness.ran_anything_with("push"));
        assert_eq!(harness.remote_hash("refs/heads/main").len(), 40);
    }

    fn forcing(mode: ForceMode, source: &str, target: &str) -> PushSpec {
        PushSpec {
            force: Some(mode),
            ..spec(source, target)
        }
    }

    // Protected branches and forcing.

    #[test]
    fn main_and_master_protected_by_default() {
        let patterns = RepoSettings::default().protected_branches;

        assert!(is_protected(&patterns, "main"));
        assert!(is_protected(&patterns, "master"));
        assert!(!is_protected(&patterns, "feature"));
    }

    #[test]
    fn pattern_is_an_anchored_regex() {
        let patterns = ["release.*".to_string(), "master".to_string()];

        assert!(is_protected(&patterns, "release-1"));
        assert!(!is_protected(&patterns, "pre-release-1"));
        assert!(is_protected(&patterns, "master"));
        assert!(!is_protected(&patterns, "master2"));
        assert!(!is_protected(&patterns, "my-master"));
    }

    // Java's `String.matches` requires the whole string in each alternative.
    #[test]
    fn pattern_with_alternatives_anchors_all_of_them() {
        let patterns = ["main|release".to_string()];

        assert!(is_protected(&patterns, "main"));
        assert!(is_protected(&patterns, "release"));
        assert!(!is_protected(&patterns, "main-2"));
        assert!(!is_protected(&patterns, "pre-release"));
    }

    #[test]
    fn invalid_pattern_does_not_break() {
        let patterns = ["(".to_string(), "a)|(b".to_string(), "main".to_string()];

        assert!(!is_protected(&patterns, "("));
        assert!(!is_protected(&patterns, "a"));
        assert!(is_protected(&patterns, "main"));
        assert_eq!(invalid_patterns(&patterns), ["(", "a)|(b"]);
    }

    #[test]
    fn a_bad_pattern_reports_the_first_error() {
        assert_eq!(
            first_pattern_error("main;(").as_deref(),
            Some("Skipped invalid protected branch pattern '(': unclosed group")
        );
        assert_eq!(
            first_pattern_error(" main ; ; a) ; ( ").as_deref(),
            Some("Skipped invalid protected branch pattern 'a)': unopened group")
        );
        assert_eq!(first_pattern_error("main; release.*;"), None);
        assert_eq!(first_pattern_error(""), None);
    }

    #[test]
    fn protected_branch_rejects_force_in_the_core() {
        let harness = Harness::new();
        let remote = harness.remote_hash("refs/heads/main");
        harness
            .repo
            .git(&["commit", "-q", "--amend", "-m", "rewritten"]);
        for force in [ForceMode::Lease, ForceMode::Plain] {
            let result = harness.push(forcing(force, "main", "main"));

            assert_eq!(
                result.status,
                PushStatus::Error("Force push to main is prohibited".to_string())
            );
        }
        assert!(!harness.ran_anything_with("push"));
        assert_eq!(harness.remote_hash("refs/heads/main"), remote);
    }

    // P§4.1: the rules arrive with a fetch, and `check_force` reads them with the user's.
    #[test]
    fn a_fetch_loads_the_remotes_rules_and_force_push_to_them_is_prohibited() {
        let harness = Harness::new();
        let github = GithubRemote::new();
        github.point(harness.path(), "origin", harness.origin.path());
        harness
            .sync
            .set_after_fetch(github.hook(harness.data.path()));
        harness.repo.git(&["checkout", "-q", "-b", "release/1"]);
        harness.repo.commit("r.txt", b"r\n", "release");
        harness
            .repo
            .git(&["push", "-q", "-u", "origin", "release/1"]);
        harness
            .repo
            .git(&["commit", "-q", "--amend", "-m", "rewritten"]);
        let remote = harness.remote_hash("refs/heads/release/1");

        github.fake.reply(rules(&["release/*"]));
        let sync = harness.sync.clone();
        let fetched = in_task(&harness.runner, harness.path(), move |ctx| {
            crate::fetch::fetch_remote(ctx, &sync, "origin")
        });
        github.fake.finish();
        assert_eq!(fetched, Ok(Vec::new()));
        assert_eq!(
            settings::try_effective_protected(harness.data.path()).unwrap(),
            ["master", "main", r"release\/.*"]
        );

        let result = harness.push(forcing(ForceMode::Plain, "release/1", "release/1"));

        assert_eq!(
            result.status,
            PushStatus::Error("Force push to release/1 is prohibited".to_string())
        );
        assert!(!harness.ran_anything_with("push"));
        assert_eq!(harness.remote_hash("refs/heads/release/1"), remote);
    }

    // Git accepts these ways of naming the target: none must skip the protection.
    #[test]
    fn naming_the_target_another_way_does_not_skip_the_protection() {
        let harness = Harness::new();
        harness
            .repo
            .git(&["commit", "-q", "--amend", "-m", "rewritten"]);
        let remote = harness.remote_hash("refs/heads/main");
        for target in ["refs/heads/main", "heads/main"] {
            let result = harness.push(forcing(ForceMode::Plain, "main", target));

            assert_ne!(result.status, PushStatus::Forced, "{target}");
            assert_eq!(harness.remote_hash("refs/heads/main"), remote, "{target}");
        }
    }

    #[test]
    fn the_repository_patterns_rule_in_the_core() {
        let harness = Harness::new();
        let old = harness.rewritten_feature();
        harness.protect(&["fea.*"]);

        let result = harness.push(forcing(ForceMode::Lease, "feature", "feature"));

        assert!(matches!(result.status, PushStatus::Error(_)));
        assert_eq!(harness.remote_hash("refs/heads/feature"), old);
    }

    #[test]
    fn without_being_able_to_read_the_patterns_nothing_is_forced() {
        let harness = Harness::new();
        let old = harness.rewritten_feature();
        let file = harness.data.path().join("repo-settings.json");
        fs::write(&file, b"{}").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();

        let result = harness.push(forcing(ForceMode::Lease, "feature", "feature"));

        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(result.status, PushStatus::Error(_)));
        assert_eq!(harness.remote_hash("refs/heads/feature"), old);
    }

    #[test]
    fn without_asking_nothing_is_forced_even_if_the_branch_is_not_protected() {
        let harness = Harness::new();
        let old = harness.rewritten_feature();

        let result = harness.push(spec("feature", "feature"));

        assert_eq!(result.status, PushStatus::RejectedNoFf);
        assert_eq!(harness.remote_hash("refs/heads/feature"), old);
        assert!(!harness.ran_anything_with("--force"));
        assert!(!harness
            .console
            .lines()
            .iter()
            .any(|line| line.command && line.text.contains("--force-with-lease")));
    }

    #[test]
    fn force_with_lease_carries_branch_and_sha() {
        let harness = Harness::new();
        let old = harness.rewritten_feature();

        let result = harness.push(forcing(ForceMode::Lease, "feature", "feature"));

        assert_eq!(result.status, PushStatus::Forced);
        assert!(harness.ran(&format!("--force-with-lease=refs/heads/feature:{old}")));
        assert_eq!(harness.remote_hash("refs/heads/feature"), harness.head());
    }

    #[test]
    fn force_with_lease_without_tracked_carries_an_empty_sha() {
        let harness = Harness::new();
        harness.repo.git(&["checkout", "-q", "-b", "new"]);

        let result = harness.push(forcing(ForceMode::Lease, "new", "new"));

        assert_eq!(result.status, PushStatus::NewBranch);
        assert!(harness.ran("--force-with-lease=refs/heads/new:"));
    }

    #[test]
    fn force_to_a_target_with_a_full_name_reads_its_tracking() {
        let harness = Harness::new();
        let old = harness.rewritten_feature();

        let result = harness.push(forcing(ForceMode::Lease, "feature", "refs/heads/feature"));

        assert_eq!(result.status, PushStatus::Forced);
        assert!(harness.ran(&format!("--force-with-lease=refs/heads/feature:{old}")));
    }

    #[test]
    fn stale_lease_is_rejected_stale() {
        let harness = Harness::new();
        harness.rewritten_feature();
        harness.someone_else_pushes_to("feature");
        let remote = harness.remote_hash("refs/heads/feature");

        let result = harness.push(forcing(ForceMode::Lease, "feature", "feature"));

        assert_eq!(result.status, PushStatus::RejectedStale);
        assert_eq!(harness.remote_hash("refs/heads/feature"), remote);
    }

    #[test]
    fn force_plain_overwrites_only_when_asked() {
        let harness = Harness::new();
        harness.rewritten_feature();
        harness.someone_else_pushes_to("feature");

        let result = harness.push(forcing(ForceMode::Plain, "feature", "feature"));

        assert_eq!(result.status, PushStatus::Forced);
        assert!(harness.ran("+refs/heads/feature:refs/heads/feature"));
        assert!(!harness.ran_anything_with("--force"));
        assert_eq!(harness.remote_hash("refs/heads/feature"), harness.head());
    }

    // A `v1` tag different on each side, with the same name as nothing that is forced.
    fn differing_remote_tag(harness: &Harness, name: &str, remote: &str) {
        let full = format!("refs/tags/{name}");
        let origin = harness.origin.path().to_str().unwrap();
        harness.repo.git(&["tag", "-f", name, remote]);
        harness.repo.git(&["push", "-q", origin, &full]);
        harness.repo.git(&["tag", "-f", name, "HEAD"]);
    }

    #[test]
    fn force_plain_with_tags_does_not_replace_remote_tags() {
        let harness = Harness::new();
        let old = harness.rewritten_feature();
        differing_remote_tag(&harness, "v1", &old);
        harness.repo.git(&["tag", "v2"]);

        let result = harness.push(PushSpec {
            tags: Some(PushTags::All),
            ..forcing(ForceMode::Plain, "feature", "feature")
        });

        assert_eq!(result.status, PushStatus::Forced);
        assert_eq!(result.tags, ["v2"]);
        assert_eq!(harness.remote_hash("refs/heads/feature"), harness.head());
        assert_eq!(harness.remote_hash("refs/tags/v1"), old);
        assert_eq!(harness.remote_hash("refs/tags/v2"), harness.head());
    }

    #[test]
    fn lease_does_not_reach_a_tag_named_like_the_branch() {
        let harness = Harness::new();
        let old = harness.rewritten_feature();
        differing_remote_tag(&harness, "feature", &old);

        let result = harness.push(PushSpec {
            tags: Some(PushTags::All),
            ..forcing(ForceMode::Lease, "feature", "feature")
        });

        assert_eq!(result.status, PushStatus::Forced);
        assert_eq!(harness.remote_hash("refs/heads/feature"), harness.head());
        assert_eq!(harness.remote_hash("refs/tags/feature"), old);
    }

    #[test]
    fn a_lease_rejection_cannot_be_resolved_by_updating() {
        let harness = Harness::new();
        harness.rewritten_feature();
        harness.someone_else_pushes_to("feature");

        let result = harness.push(forcing(ForceMode::Lease, "feature", "feature"));

        assert_eq!(result.updatable, None);
    }

    // Retrying after updating.

    #[test]
    fn retry_after_updating_pushes() {
        let harness = Harness::new();
        harness.someone_else_pushes();
        harness.repo.commit("mine.txt", b"m\n", "own");
        let rejected = harness.push(spec("main", "main"));
        assert_eq!(
            rejected.updatable,
            Some(Updatable {
                branch: "main".to_string(),
                merges: false,
            })
        );

        let result = harness.update_and_push(spec("main", "main"), UpdateMethod::Merge);

        assert_eq!(result.push.status, PushStatus::Success);
        assert_eq!(result.update.unwrap().outcome, Outcome::Success);
        assert_eq!(result.attempts, 2);
        assert_eq!(harness.remote_hash("refs/heads/main"), harness.head());
        assert!(harness.path().join("theirs.txt").exists());
        assert!(!harness.ran_anything_with("--force"));
    }

    #[test]
    fn a_conflict_while_updating_stops_and_does_not_push_again() {
        let harness = Harness::new();
        harness.someone_else_pushes();
        harness.repo.commit("theirs.txt", b"mine\n", "own");
        let remote = harness.remote_hash("refs/heads/main");

        let result = harness.update_and_push(spec("main", "main"), UpdateMethod::Merge);

        let update = result.update.unwrap();
        assert_eq!(update.outcome, Outcome::Conflicts);
        assert!(update.in_progress);
        assert_eq!(result.push.status, PushStatus::RejectedNoFf);
        assert_eq!(result.attempts, 1);
        assert_eq!(harness.remote_hash("refs/heads/main"), remote);
    }

    fn rejected() -> PushResult {
        result_of(&spec("main", "main"), PushStatus::RejectedNoFf)
    }

    fn updated(commits: u32, range: (&str, &str)) -> UpdateResult {
        UpdateResult {
            commits,
            files: commits,
            range: Some((range.0.to_string(), range.1.to_string())),
            ..Outcome::Success.into()
        }
    }

    #[test]
    fn retries_limited_to_10() {
        let (mut updates, mut pushes) = (0, 0);

        let result = retrying(
            rejected(),
            1,
            |_| {
                updates += 1;
                Some(updated(1, ("a", "b")))
            },
            || {
                pushes += 1;
                Ok(rejected())
            },
        );

        assert_eq!((updates, pushes), (9, 9));
        assert_eq!(result.attempts, MAX_PUSH_ATTEMPTS);
        assert_eq!(result.push.status, PushStatus::RejectedNoFf);
    }

    #[test]
    fn the_retry_count_continues_after_asking() {
        let mut pushes = 0;

        let result = retrying(
            rejected(),
            8,
            |_| Some(updated(1, ("a", "b"))),
            || {
                pushes += 1;
                Ok(rejected())
            },
        );

        assert_eq!(pushes, 2);
        assert_eq!(result.attempts, MAX_PUSH_ATTEMPTS);
    }

    #[test]
    fn what_was_received_adds_up_across_retries() {
        let mut ranges = vec![("b", "c"), ("a", "b")];
        let mut statuses = vec![PushStatus::Success, PushStatus::RejectedNoFf];

        let result = retrying(
            rejected(),
            1,
            |_| Some(updated(2, ranges.pop().unwrap())),
            || Ok(result_of(&spec("main", "main"), statuses.pop().unwrap())),
        );

        let update = result.update.unwrap();
        assert_eq!(update.commits, 4);
        assert_eq!(update.range, Some(("a".to_string(), "c".to_string())));
    }

    #[test]
    fn only_the_first_update_uses_the_fetch_already_done() {
        let mut seen = Vec::new();

        retrying(
            rejected(),
            8,
            |first| {
                seen.push(first);
                Some(updated(1, ("a", "b")))
            },
            || Ok(rejected()),
        );

        assert_eq!(seen, [true, false]);
    }

    fn saved_with(restore: RestoreOutcome) -> UpdateResult {
        UpdateResult {
            saved: Some(Saved::stash("abc".to_string(), "saved".to_string())),
            restore: Some(restore),
            ..updated(1, ("a", "b"))
        }
    }

    #[test]
    fn with_changes_still_saved_it_does_not_push_again() {
        for restore in [
            RestoreOutcome::Conflicts,
            RestoreOutcome::Failed("bad".to_string()),
            RestoreOutcome::RestoreBlocked {
                files: vec!["a.txt".to_string()],
            },
        ] {
            let result = retrying(
                rejected(),
                1,
                |_| Some(saved_with(restore.clone())),
                || panic!("must not push"),
            );

            assert_eq!(result.push.status, PushStatus::RejectedNoFf);
            assert_eq!(result.update.unwrap().restore, Some(restore));
        }
    }

    // Git was left half-done and restoring was not attempted: an empty `restore` also means still saved.
    #[test]
    fn without_attempting_to_restore_it_also_stays_saved() {
        let update = UpdateResult {
            restore: None,
            ..saved_with(RestoreOutcome::Conflicts)
        };

        let result = retrying(
            rejected(),
            1,
            |_| Some(update.clone()),
            || panic!("must not push"),
        );

        assert_eq!(result.update.unwrap().saved, update.saved);
    }

    // The same rule as `stillSaved` in `src/update/flow.ts`.
    #[test]
    fn what_was_returned_to_disk_lets_it_continue_and_its_notice_is_not_lost() {
        for restore in [
            RestoreOutcome::Restored,
            RestoreOutcome::RestoredWithoutIndex,
            RestoreOutcome::Gone,
        ] {
            let mut updates = vec![updated(1, ("b", "c")), saved_with(restore.clone())];
            let mut statuses = vec![PushStatus::Success, PushStatus::RejectedNoFf];

            let result = retrying(
                rejected(),
                1,
                |_| updates.pop(),
                || Ok(result_of(&spec("main", "main"), statuses.pop().unwrap())),
            );

            let update = result.update.unwrap();
            assert_eq!(result.push.status, PushStatus::Success);
            assert_eq!(update.restore, Some(restore));
            assert!(update.saved.is_some());
        }
    }

    #[test]
    fn a_stage_notice_is_not_overwritten_by_a_later_restored() {
        let mut updates = vec![
            saved_with(RestoreOutcome::Restored),
            saved_with(RestoreOutcome::RestoredWithoutIndex),
        ];
        let mut statuses = vec![PushStatus::Success, PushStatus::RejectedNoFf];

        let result = retrying(
            rejected(),
            1,
            |_| updates.pop(),
            || Ok(result_of(&spec("main", "main"), statuses.pop().unwrap())),
        );

        assert_eq!(
            result.update.unwrap().restore,
            Some(RestoreOutcome::RestoredWithoutIndex)
        );
    }

    #[test]
    fn cancelling_the_push_keeps_what_the_update_did() {
        let result = retrying(
            rejected(),
            1,
            |_| Some(saved_with(RestoreOutcome::Restored)),
            || Err(GitError::Cancelled),
        );

        let update = result.update.unwrap();
        assert_eq!(update.commits, 1);
        assert!(update.saved.is_some());
        assert_eq!(result.push.status, PushStatus::RejectedNoFf);
        assert_eq!(result.attempts, 1);
    }

    #[test]
    fn a_failure_while_pushing_keeps_what_the_update_did() {
        let result = retrying(
            rejected(),
            1,
            |_| Some(updated(2, ("a", "b"))),
            || Err(GitError::Io("bad".to_string())),
        );

        assert_eq!(result.update.unwrap().commits, 2);
        assert!(matches!(result.push.status, PushStatus::Error(_)));
    }

    #[test]
    fn an_update_that_does_not_end_well_does_not_push_again() {
        for outcome in [
            Outcome::Conflicts,
            Outcome::Cancelled,
            Outcome::RebasingMergeCommits {
                branch: "main".to_string(),
            },
            Outcome::RebasingPublishedCommits {
                branch: "main".to_string(),
                protected: "origin/main".to_string(),
            },
            Outcome::Error("bad".to_string()),
            Outcome::NotReady("bad".to_string()),
        ] {
            let result = retrying(
                rejected(),
                1,
                |_| Some(outcome.clone().into()),
                || panic!("must not push"),
            );

            assert_eq!(result.push.status, PushStatus::RejectedNoFf);
            assert_eq!(result.update.unwrap().outcome, outcome);
        }
    }

    #[test]
    fn it_does_not_retry_on_force_or_on_a_non_current_branch() {
        let harness = Harness::new();
        harness.repo.git(&["checkout", "-q", "-b", "other"]);
        harness.repo.commit("o.txt", b"o\n", "other");
        harness.repo.git(&["push", "-q", "-u", "origin", "other"]);
        harness.someone_else_pushes_to("other");
        harness.repo.commit("p.txt", b"p\n", "own");
        harness.repo.git(&["checkout", "-q", "main"]);
        let head = harness.head();

        let other = harness.push(spec("other", "other"));
        let retried = harness.update_and_push(spec("other", "other"), UpdateMethod::Merge);
        let forced = harness.update_and_push(
            forcing(ForceMode::Lease, "main", "main"),
            UpdateMethod::Merge,
        );

        assert_eq!(other.status, PushStatus::RejectedNoFf);
        assert_eq!(other.updatable, None);
        assert_eq!(retried.update, None);
        assert_eq!(forced.update, None);
        assert_eq!(forced.push.status, PushStatus::RejectedNoFf);
        assert_eq!(harness.head(), head);
        assert!(!harness.ran_anything_with("merge"));
    }

    #[test]
    fn it_does_not_retry_towards_a_target_that_is_not_the_tracked_one() {
        let harness = Harness::new();
        harness.repo.git(&["push", "-q", "origin", "main:other"]);
        harness.someone_else_pushes_to("other");
        harness.repo.commit("p.txt", b"p\n", "own");

        let result = harness.push(spec("main", "other"));

        assert_eq!(result.status, PushStatus::RejectedNoFf);
        assert_eq!(result.updatable, None);
    }

    #[test]
    fn a_loose_commit_that_is_rejected_does_not_retry() {
        let harness = Harness::new();
        harness.someone_else_pushes();
        harness.repo.commit("p.txt", b"p\n", "own");
        let head = harness.head();

        let result = harness.push(spec(&head, "main"));

        assert_eq!(result.status, PushStatus::RejectedNoFf);
        assert_eq!(result.updatable, None);
    }

    #[test]
    fn unpushed_merges_are_reported_on_rejection() {
        let harness = Harness::new();
        harness.repo.git(&["checkout", "-q", "-b", "side"]);
        harness.repo.commit("a.txt", b"side\n", "side");
        harness.repo.git(&["checkout", "-q", "main"]);
        harness.repo.commit("a.txt", b"main\n", "main");
        // `git log -c` only shows the merges that resolved a conflict.
        let _ = crate::test_support::git_command()
            .arg("-C")
            .arg(harness.path())
            .args(["merge", "-q", "side"])
            .output()
            .unwrap();
        fs::write(harness.path().join("a.txt"), b"juntos\n").unwrap();
        harness.repo.git(&["add", "a.txt"]);
        harness.repo.git(&["commit", "-q", "-m", "merge"]);
        harness.someone_else_pushes();

        let result = harness.push(spec("main", "main"));

        assert!(result.updatable.unwrap().merges);
    }

    #[test]
    fn a_rejection_does_not_become_forced() {
        let harness = Harness::new();
        harness.someone_else_pushes();
        harness.repo.commit("mine.txt", b"m\n", "own");
        let remote = harness.remote_hash("refs/heads/main");

        let result = harness.push(spec("main", "main"));

        assert_eq!(result.status, PushStatus::RejectedNoFf);
        assert_eq!(harness.remote_hash("refs/heads/main"), remote);
        assert!(!harness.ran_anything_with("--force"));
    }

    // The parser.

    #[test]
    fn parser_success() {
        let output = "To /tmp/origin\n \trefs/heads/main:refs/heads/main\tabc1234..def5678\nDone\n";

        let lines = parse_porcelain(output);

        assert_eq!(
            lines,
            [PorcelainLine {
                flag: ' ',
                from: "refs/heads/main".to_string(),
                to: "refs/heads/main".to_string(),
                summary: "abc1234..def5678".to_string(),
                reason: None,
                range: Some("abc1234..def5678".to_string()),
            }]
        );
    }

    #[test]
    fn parser_new_branch_up_to_date_and_forced() {
        let flags = |text: &str| -> Vec<(char, Option<String>)> {
            parse_porcelain(text)
                .into_iter()
                .map(|line| (line.flag, line.range))
                .collect()
        };

        assert_eq!(flags("*\ta:b\t[new branch]\n"), [('*', None)]);
        assert_eq!(flags("=\ta:b\t[up to date]\n"), [('=', None)]);
        assert_eq!(
            flags("+\ta:b\tabc...def (forced update)\n"),
            [('+', Some("abc...def".to_string()))]
        );
    }

    #[test]
    fn parser_non_fast_forward_rejection() {
        let lines =
            parse_porcelain("!\trefs/heads/a:refs/heads/a\t[rejected] (non-fast-forward)\n");

        assert_eq!(lines[0].summary, "[rejected]");
        assert_eq!(lines[0].reason.as_deref(), Some("non-fast-forward"));
        assert_eq!(status_of(&lines[0]), PushStatus::RejectedNoFf);
    }

    #[test]
    fn parser_fetch_first_rejection() {
        let lines = parse_porcelain("!\trefs/heads/a:refs/heads/a\t[rejected] (fetch first)\n");

        assert_eq!(status_of(&lines[0]), PushStatus::RejectedNoFf);
    }

    #[test]
    fn parser_lock_rejection_is_like_non_fast_forward() {
        let lines =
            parse_porcelain("!\trefs/heads/a:refs/heads/a\t[remote rejected] (failed to lock)\n");

        assert_eq!(status_of(&lines[0]), PushStatus::RejectedNoFf);
    }

    #[test]
    fn parser_stale_info() {
        let lines = parse_porcelain("!\trefs/heads/a:refs/heads/a\t[rejected] (stale info)\n");

        assert_eq!(status_of(&lines[0]), PushStatus::RejectedStale);
    }

    #[test]
    fn parser_other_rejection_carries_its_reason() {
        let lines = parse_porcelain(
            "!\trefs/heads/a:refs/heads/a\t[remote rejected] (pre-receive hook declined)\n",
        );

        assert_eq!(
            status_of(&lines[0]),
            PushStatus::RejectedOther("pre-receive hook declined".to_string())
        );
    }

    #[test]
    fn parser_without_a_line_uses_stderr() {
        let harness = Harness::new();
        harness
            .repo
            .git(&["remote", "set-url", "origin", "/does/not/exist"]);
        harness.repo.commit("b.txt", b"b\n", "two");
        assert_eq!(parse_porcelain("To /tmp\nDone\n"), []);

        let result = harness.push(spec("main", "main"));

        let PushStatus::Error(text) = &result.status else {
            panic!("{:?}", result.status);
        };
        assert!(text.contains("/does/not/exist"), "{text}");
    }

    // Against a real remote.

    #[test]
    fn up_to_date_branch_gives_up_to_date() {
        let harness = Harness::new();

        let result = harness.push(spec("main", "main"));

        assert_eq!(result.status, PushStatus::UpToDate);
        assert_eq!(result.pushed, 0);
    }

    #[test]
    fn real_push_to_a_bare_remote() {
        let harness = Harness::new();
        harness.repo.commit("b.txt", b"b\n", "two");
        let head = harness.repo.commit("c.txt", b"c\n", "three");

        let result = harness.push(spec("main", "main"));

        assert_eq!(result.status, PushStatus::Success);
        assert_eq!(result.pushed, 2);
        assert_eq!(result.source, "main");
        assert_eq!(result.destination, "origin/main");
        assert_eq!(harness.remote_hash("refs/heads/main"), head);
    }

    #[test]
    fn successful_push_marks_the_remote_as_authenticated() {
        let harness = Harness::new();
        assert!(!harness.sync.is_authenticated(harness.path(), "origin"));

        harness.push(spec("main", "main"));

        assert!(harness.sync.is_authenticated(harness.path(), "origin"));
    }

    #[test]
    fn rejected_push_does_not_mark_the_remote() {
        let harness = Harness::new();
        harness.someone_else_pushes();
        harness.repo.commit("mine.txt", b"m\n", "own");

        harness.push(spec("main", "main"));

        assert!(!harness.sync.is_authenticated(harness.path(), "origin"));
    }

    #[test]
    fn the_rejection_of_a_remote_hook_carries_its_reason() {
        let harness = Harness::new();
        let hook = harness.origin.path().join("hooks/pre-receive");
        fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        harness.repo.commit("b.txt", b"b\n", "two");

        let result = harness.push(spec("main", "main"));

        assert_eq!(
            result.status,
            PushStatus::RejectedOther("pre-receive hook declined".to_string())
        );
    }

    #[test]
    fn push_and_fetch_go_through_the_turn_of_their_remote() {
        assert_eq!(
            task_kind(&spec("main", "main")),
            TaskKind::Remote("origin".to_string())
        );
    }

    // What goes out.

    #[test]
    fn outgoing_commits_of_a_tracked_branch() {
        let harness = Harness::new();
        let first = harness.repo.commit("b.txt", b"b\n", "two");
        let second = harness.repo.commit("c.txt", b"c\n", "three");

        let commits = harness.outgoing("main", "main").unwrap();

        let hashes: Vec<&str> = commits.iter().map(|commit| commit.hash.as_str()).collect();
        assert_eq!(hashes, [second.as_str(), first.as_str()]);
        assert_eq!(commits[0].subject, "three");
        assert_eq!(commits[0].author, "Test");
    }

    #[test]
    fn outgoing_commits_of_a_new_branch() {
        let harness = Harness::new();
        harness.repo.git(&["checkout", "-q", "-b", "x"]);
        let mine = harness.repo.commit("b.txt", b"b\n", "two");
        // What is already on any branch of the remote does not go out, even if it is not the target one.
        harness.repo.git(&["remote", "add", "another", "/does/not/matter"]);

        let commits = harness.outgoing("x", "x").unwrap();

        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].hash, mine);
        let args = outgoing_args("refs/heads/x", "origin", "x", true).join(" ");
        assert!(
            args.ends_with("refs/heads/x --not --remotes=origin --max-count=1000 --"),
            "{args}"
        );
    }

    #[test]
    fn outgoing_commits_up_to_a_commit() {
        let harness = Harness::new();
        let first = harness.repo.commit("b.txt", b"b\n", "two");
        harness.repo.commit("c.txt", b"c\n", "three");

        let commits = harness.outgoing(&first, "main").unwrap();

        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].hash, first);
    }

    #[test]
    fn outgoing_commits_do_not_let_options_through() {
        let harness = Harness::new();
        for (source, target) in [("--all", "main"), ("main", "--all"), ("main", "+x")] {
            assert!(
                harness.outgoing(source, target).is_err(),
                "{source} {target}"
            );
        }
    }

    #[test]
    fn the_branch_of_a_commit_is_the_current_one_if_it_contains_it() {
        let harness = Harness::new();
        let head = harness.head();
        harness.repo.git(&["branch", "-q", "aaa"]);

        assert_eq!(
            branch_of_commit(harness.path(), &head).unwrap().as_deref(),
            Some("main")
        );
    }

    #[test]
    fn the_branch_of_a_commit_foreign_to_head_is_a_local_one_that_contains_it() {
        let harness = Harness::new();
        harness.repo.git(&["checkout", "-q", "-b", "other"]);
        let hash = harness.repo.commit("b.txt", b"b\n", "two");
        harness.repo.git(&["checkout", "-q", "main"]);

        assert_eq!(
            branch_of_commit(harness.path(), &hash).unwrap().as_deref(),
            Some("other")
        );
    }

    #[test]
    fn a_commit_without_a_local_branch_has_nothing_to_push_to() {
        let harness = Harness::new();
        harness.repo.git(&["checkout", "-q", "-b", "other"]);
        let hash = harness.repo.commit("b.txt", b"b\n", "two");
        harness.repo.git(&["checkout", "-q", "main"]);
        harness.repo.git(&["branch", "-q", "-D", "other"]);

        assert_eq!(branch_of_commit(harness.path(), &hash).unwrap(), None);
        assert!(branch_of_commit(harness.path(), "--all").is_err());
    }

    // What the dialog needs.

    #[test]
    fn info_brings_what_the_dialog_shows() {
        let harness = Harness::new();
        harness.repo.git(&["branch", "-q", "loose"]);

        let found = info(harness.path(), RepoSettings::default(), Some("loose")).unwrap();

        let name = harness.path().file_name().unwrap().to_str().unwrap();
        assert_eq!(found.repository, name);
        assert_eq!(found.branch.as_deref(), Some("loose"));
        assert_eq!(found.head.as_deref(), Some(harness.head().as_str()));
        assert_eq!(found.remotes, ["origin"]);
        assert_eq!(found.remote_branches, ["origin/main"]);
        assert_eq!(found.target.branch, "loose");
        assert!(found.target.is_new);
        assert!(!found.pre_push_hook);

        harness.hook();
        assert!(
            info(harness.path(), RepoSettings::default(), None)
                .unwrap()
                .pre_push_hook
        );
    }

    #[test]
    fn info_without_a_branch_is_the_current_one() {
        let harness = Harness::new();

        assert_eq!(
            info(harness.path(), RepoSettings::default(), None)
                .unwrap()
                .branch
                .as_deref(),
            Some("main")
        );

        harness.repo.git(&["checkout", "-q", "--detach"]);
        assert_eq!(
            info(harness.path(), RepoSettings::default(), None)
                .unwrap()
                .branch,
            None
        );
    }

    #[test]
    fn info_of_a_repository_without_commits_has_no_head() {
        let repo = TempRepo::new();

        assert_eq!(
            info(repo.path(), RepoSettings::default(), None)
                .unwrap()
                .head,
            None
        );
    }

    #[test]
    fn info_does_not_let_an_option_through() {
        let harness = Harness::new();

        assert!(info(harness.path(), RepoSettings::default(), Some("--all")).is_err());
    }

    // What is remembered.

    #[test]
    fn the_tags_choice_belongs_to_each_repository() {
        let (one, other) = (TempDir::new().unwrap(), TempDir::new().unwrap());
        assert_eq!(read_tags(one.path()), None);

        write_tags(one.path(), Some(PushTags::CurrentBranch)).unwrap();

        assert_eq!(read_tags(one.path()), Some(PushTags::CurrentBranch));
        assert_eq!(read_tags(other.path()), None, "the other's does not carry over");

        write_tags(one.path(), None).unwrap();
        assert_eq!(read_tags(one.path()), None);
    }

    #[test]
    fn a_damaged_choice_is_none() {
        let dir = TempDir::new().unwrap();
        for text in ["{ broken", "\"nonsense\"", "3", ""] {
            fs::write(dir.path().join("push-tags.json"), text).unwrap();
            assert_eq!(read_tags(dir.path()), None, "{text}");
        }
    }

    // For `docs/reports/phase-2-commands.md`: records what the app launches in checkout, new
    // branch, fetch, update, pull, push and force push. It is run with `scripts/git-wrapper.sh` as
    // `git`; the log is only active while each operation runs, not during the preparation.
    #[test]
    #[ignore = "only to record commands with scripts/git-wrapper.sh"]
    fn phase_2_trace() {
        use crate::branches::{CheckoutMode, Overwrite, RemoteOptions};
        use crate::pull::PullOption;
        use std::io::Write;

        fn in_repo<T: Send + 'static>(
            harness: &Harness,
            kind: TaskKind,
            work: impl FnOnce(&Repo) -> T + Send + 'static,
        ) -> T {
            let (send, receive) = mpsc::channel();
            let path = harness.path().to_path_buf();
            let data = harness.data.path().to_path_buf();
            harness
                .runner
                .start(harness.path(), kind, "trace".to_string(), move |ctx| {
                    let repo = Repo {
                        ctx,
                        path: &path,
                        data: &data,
                    };
                    send.send(work(&repo)).unwrap();
                    Ok(())
                });
            receive.recv_timeout(WAIT).unwrap()
        }

        let log = std::env::var_os("RAMA_GIT_LOG").unwrap();
        fs::write(&log, "").unwrap();
        // The log is only activated around what is measured.
        let section = |title: &str| {
            let mut file = fs::OpenOptions::new().append(true).open(&log).unwrap();
            writeln!(file, "## {title}").unwrap();
            std::env::set_var("RAMA_GIT_LOG", &log);
        };
        let silence = || std::env::remove_var("RAMA_GIT_LOG");
        silence();

        let harness = Harness::new();
        let harness_origin = harness.origin.path().to_path_buf();
        // A different clone pushes a commit to main: each time with its own file.
        let outsider = |file: &str, contents: &str| {
            let colleague = TempRepo::new();
            let url = harness_origin.to_str().unwrap().to_string();
            colleague.git(&["remote", "add", "origin", &url]);
            colleague.git(&["fetch", "-q", "origin"]);
            colleague.git(&["checkout", "-q", "-B", "main", "origin/main"]);
            colleague.commit(file, contents.as_bytes(), &format!("from outside: {file}"));
            colleague.git(&["push", "-q", "origin", "main"]);
        };
        let repo = &harness.repo;
        repo.git(&["branch", "side"]);
        repo.git(&["checkout", "-q", "side"]);
        repo.commit("a.txt", b"1\n2\n", "side");
        repo.git(&["checkout", "-q", "-b", "remote_branch", "main"]);
        repo.commit("r.txt", b"r\n", "remote_branch");
        repo.git(&["push", "-q", "origin", "remote_branch"]);
        repo.git(&["checkout", "-q", "main"]);
        repo.git(&["branch", "-D", "remote_branch"]);
        outsider("theirs1.txt", "1\n");

        let plain = |target: &'static str| {
            move |repo: &Repo| branches::checkout(repo, target, CheckoutMode::Plain)
        };

        section("checkout of a local branch, clean");
        let done = in_repo(&harness, TaskKind::Write, plain("side"));
        silence();
        assert!(done.is_ok(), "{done:?}");
        harness.repo.git(&["checkout", "-q", "main"]);

        fs::write(harness.path().join("a.txt"), "0\n1\n").unwrap();
        section("smart checkout with local changes that git does not let be overwritten");
        let done = in_repo(&harness, TaskKind::Write, |repo| {
            branches::checkout(repo, "side", CheckoutMode::Smart)
        });
        silence();
        assert!(done.is_ok(), "{done:?}");
        harness.repo.git(&["checkout", "-q", "--", "a.txt"]);
        harness.repo.git(&["checkout", "-q", "main"]);

        fs::write(harness.path().join("a.txt"), "0\n1\n").unwrap();
        section("force checkout");
        let done = in_repo(&harness, TaskKind::Write, |repo| {
            branches::checkout(repo, "side", CheckoutMode::Force)
        });
        silence();
        assert!(done.is_ok(), "{done:?}");
        harness.repo.git(&["checkout", "-q", "main"]);

        section("checkout of a remote branch");
        let done = in_repo(&harness, TaskKind::Write, |repo| {
            branches::checkout_remote(repo, "origin/remote_branch", RemoteOptions::default())
        });
        silence();
        assert!(done.is_ok(), "{done:?}");
        harness.repo.git(&["checkout", "-q", "main"]);

        section("new branch from another, with checkout");
        let done = in_repo(&harness, TaskKind::Write, |repo| {
            branches::create(
                repo,
                "new",
                Some("main"),
                true,
                Overwrite::No,
                CheckoutMode::Plain,
            )
        });
        silence();
        assert!(done.is_ok(), "{done:?}");
        harness.repo.git(&["checkout", "-q", "main"]);

        section("new branch without checkout");
        let done = in_repo(&harness, TaskKind::Write, |repo| {
            branches::create(
                repo,
                "loose",
                None,
                false,
                Overwrite::No,
                CheckoutMode::Plain,
            )
        });
        silence();
        assert!(done.is_ok(), "{done:?}");

        section("fetch");
        let sync = harness.sync.clone();
        let fetched = in_repo(&harness, TaskKind::Read, move |repo| {
            fetch::fetch_all(repo.ctx, &sync)
        });
        silence();
        assert!(fetched.is_ok(), "{fetched:?}");

        // Update by merge: divergent histories and a local change on a file that arrives.
        harness.repo.commit("own.txt", b"p\n", "own");
        outsider("a.txt", "1\n2\n3\n");
        fs::write(harness.path().join("a.txt"), "0\n1\n").unwrap();
        section("update by merge, with overlapping local changes");
        let sync = harness.sync.clone();
        let updated = in_repo(&harness, TaskKind::Read, move |repo| {
            update::update_project(
                repo,
                &sync,
                UpdateMethod::Merge,
                UpdateOptions::default(),
                || Ok(()),
            )
        });
        silence();
        assert_eq!(updated.outcome, Outcome::Success, "{updated:?}");

        // Update by rebase: any local change is saved.
        harness.repo.commit("propio2.txt", b"p\n", "own 2");
        outsider("theirs3.txt", "3\n");
        fs::write(harness.path().join("a.txt"), "00\n0\n1\n").unwrap();
        section("update by rebase, with local changes");
        let sync = harness.sync.clone();
        let updated = in_repo(&harness, TaskKind::Read, move |repo| {
            update::update_project(
                repo,
                &sync,
                UpdateMethod::Rebase,
                UpdateOptions::default(),
                || Ok(()),
            )
        });
        silence();
        assert_eq!(updated.outcome, Outcome::Success, "{updated:?}");
        harness.repo.git(&["checkout", "-q", "--", "a.txt"]);

        let spec_of = |source: &str| spec(source, source);
        harness.repo.commit("push.txt", b"s\n", "to push");
        let push_spec = spec_of("main");
        section("push of the current branch");
        let pushed = harness.push(push_spec);
        silence();
        assert_eq!(pushed.status, PushStatus::Success, "{pushed:?}");

        // Pull without options: git fetches and merges.
        outsider("theirs4.txt", "4\n");
        let sync = harness.sync.clone();
        section("pull without options");
        let pulled = in_repo(&harness, TaskKind::Read, move |repo| {
            pull::pull(repo, &sync, "origin", "main", &[], || Ok(()))
        });
        silence();
        assert_eq!(pulled.outcome, Outcome::Success, "{pulled:?}");

        outsider("theirs5.txt", "5\n");
        let sync = harness.sync.clone();
        harness.repo.commit("propio3.txt", b"p\n", "own 3");
        section("pull with --rebase (goes through Update)");
        let pulled = in_repo(&harness, TaskKind::Read, move |repo| {
            pull::pull(repo, &sync, "origin", "main", &[PullOption::Rebase], || {
                Ok(())
            })
        });
        silence();
        assert_eq!(pulled.outcome, Outcome::Success, "{pulled:?}");

        harness.repo.git(&["checkout", "-q", "new"]);
        harness.repo.commit("new.txt", b"n\n", "on new");
        section("push of a new branch");
        let pushed = harness.push(spec_of("new"));
        silence();
        assert_eq!(pushed.status, PushStatus::NewBranch, "{pushed:?}");

        harness
            .repo
            .git(&["commit", "-q", "--amend", "-m", "rewritten"]);
        section("push rejected after rewriting the history");
        let rejected = harness.push(spec_of("new"));
        silence();
        assert!(
            matches!(
                rejected.status,
                PushStatus::RejectedNoFf | PushStatus::RejectedOther(_)
            ),
            "{rejected:?}"
        );

        let mut forced = spec_of("new");
        forced.force = Some(ForceMode::Lease);
        section("force push with lease");
        let pushed = harness.push(forced);
        silence();
        assert_eq!(pushed.status, PushStatus::Forced, "{pushed:?}");

        let mut forced = spec_of("new");
        forced.force = Some(ForceMode::Plain);
        harness
            .repo
            .git(&["commit", "-q", "--amend", "-m", "again"]);
        section("force push without lease");
        let pushed = harness.push(forced);
        silence();
        assert_eq!(pushed.status, PushStatus::Forced, "{pushed:?}");

        // Rejection of a normal push and retry with Update.
        harness.repo.git(&["checkout", "-q", "main"]);
        outsider("theirs6.txt", "6\n");
        harness.repo.commit("another.txt", b"o\n", "another own");
        section("retry of a rejected push: Update by merge and push");
        let retried = harness.update_and_push(spec_of("main"), UpdateMethod::Merge);
        silence();
        assert!(retried.update.is_some(), "{retried:?}");
    }
}
