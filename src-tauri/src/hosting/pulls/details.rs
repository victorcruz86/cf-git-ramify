// A pull request's details, its status and its commits: GHGQLRequests.PullRequest (findOne,
// mergeabilityData, commits), GHPRDetailsServiceImpl, GHPRStateServiceImpl,
// GHPRMergeabilityStateBuilder and GHPRChangesServiceImpl.buildCommitsTree with
// GHPRChangesDataProviderImpl.loadCommitsFromApi (plugins/github at idea/241.14494.240, P§6).
// Every text GitHub sends is kept as text; the HTML fields of the queries are never read.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::super::http::{Check, Client, HostingError, PARSE_FAILED};
use super::list::Nodes;
use super::reviewers::{self, RawActor, RawReview, RawReviewRequest, Reviewer};
use super::Session;

const DETAILS_QUERY: &str = include_str!("../graphql/findPullRequest.graphql");
const MERGEABILITY_QUERY: &str = include_str!("../graphql/findPullRequestMergeability.graphql");
const COMMITS_QUERY: &str = include_str!("../graphql/pullRequestCommits.graphql");
// GHSchemaPreview: `findOne` and `mergeabilityData` (P§6.1); `commits` sends none.
const DETAILS_ACCEPT: &str = "application/vnd.github.shadow-cat-preview+json";
const MERGEABILITY_ACCEPT: &str =
    "application/vnd.github.antiope-preview+json,application/vnd.github.merge-info-preview+json";
// GHPRChangesServiceImpl.loadCommitsFromApi: `pageSize = 100`.
const COMMITS_PAGE_SIZE: u32 = 100;
// Decision 5: the ghost user's login.
const GHOST: &str = "ghost";

// The head repository, as Task 9's checkout reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeadRepository {
    pub owner: String,
    pub is_fork: bool,
    pub name_with_owner: String,
    pub url: String,
    pub ssh_url: String,
}

// `GHPullRequest`, as far as the details panel and the checkout read it. `body` is GitHub's
// Markdown, shown as plain text; `reviewers` is P§6.4's map in its order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestDetails {
    pub number: u64,
    pub url: String,
    pub title: String,
    pub body: String,
    pub state: String,
    pub is_draft: bool,
    pub base_ref_name: String,
    pub base_ref_oid: String,
    pub head_ref_name: String,
    pub head_ref_oid: String,
    pub head_repository: Option<HeadRepository>,
    pub reviewers: Vec<Reviewer>,
}

// `CodeReviewCIJobState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum JobState {
    Failed,
    Pending,
    Skipped,
    Success,
}

// `CodeReviewCIJob`: a status context or a check run of the last commit. `url` is GitHub's, which
// the interface opens only on the connection's server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CiJob {
    pub name: String,
    pub state: JobState,
    pub is_required: bool,
    pub url: Option<String>,
}

// `GHPRMergeabilityState`, without what only the merge actions read. `headRefOid` is the head the
// status was built for. `hasConflicts` is none while GitHub does not know. `requiredApprovals` is
// already zero unless the pull request is blocked by them alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestStatus {
    pub head_ref_oid: String,
    pub jobs: Vec<CiJob>,
    pub has_conflicts: Option<bool>,
    pub required_approvals: u32,
    pub is_restricted: bool,
}

// `GHCommit`, for the commits dropdown: the plain headline (never `messageHeadlineHTML`), the
// author's presentable name and GitHub's ISO 8601 `committedDate`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestCommit {
    pub oid: String,
    pub abbreviated_oid: String,
    pub headline: String,
    pub author: String,
    pub committed_date: String,
    pub parents: Vec<String>,
    pub url: String,
}

// `GHRefUpdateRule`, as the builder reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefUpdateRule {
    pub required_approving_review_count: Option<u32>,
    #[serde(default)]
    pub required_status_check_contexts: Vec<Option<String>>,
    pub viewer_can_push: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawBaseRef {
    ref_update_rule: Option<RefUpdateRule>,
}

#[derive(Deserialize)]
struct RawOwner {
    login: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawRepository {
    owner: RawOwner,
    name_with_owner: String,
    is_fork: bool,
    url: String,
    ssh_url: String,
}

// The `pullRequestInfo` fragment, as far as it is read.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawDetails {
    number: u64,
    url: String,
    title: String,
    #[serde(default)]
    body: String,
    state: String,
    is_draft: bool,
    author: Option<RawActor>,
    #[serde(default)]
    review_requests: Nodes<RawReviewRequest>,
    #[serde(default)]
    reviews: Nodes<RawReview>,
    base_ref: Option<RawBaseRef>,
    base_ref_name: String,
    base_ref_oid: String,
    head_ref_name: String,
    head_ref_oid: String,
    head_repository: Option<RawRepository>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawContext {
    context: String,
    state: String,
    is_required: bool,
    target_url: Option<String>,
}

#[derive(Deserialize)]
struct RawStatus {
    #[serde(default)]
    contexts: Vec<RawContext>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawCheckRun {
    name: String,
    conclusion: Option<String>,
    is_required: bool,
    details_url: Option<String>,
    url: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawCheckSuite {
    #[serde(default)]
    check_runs: Nodes<RawCheckRun>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawCheckedCommit {
    status: Option<RawStatus>,
    #[serde(default)]
    check_suites: Nodes<RawCheckSuite>,
}

#[derive(Deserialize)]
struct RawPullRequestCommit {
    commit: RawCheckedCommit,
}

// The `pullRequestMergeability` fragment.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMergeability {
    mergeable: String,
    merge_state_status: String,
    #[serde(default)]
    commits: Nodes<RawPullRequestCommit>,
}

#[derive(Deserialize)]
struct RawHash {
    oid: String,
}

#[derive(Deserialize)]
struct RawGitActor {
    user: Option<RawActor>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawCommit {
    oid: String,
    abbreviated_oid: String,
    url: String,
    message_headline: String,
    committed_date: String,
    author: Option<RawGitActor>,
    #[serde(default)]
    parents: Nodes<RawHash>,
}

#[derive(Deserialize)]
struct RawCommitNode {
    commit: RawCommit,
}

fn parse_failed() -> HostingError {
    HostingError::Io(PARSE_FAILED.to_string())
}

fn variables(session: &Session, number: u64) -> Value {
    json!({
        "repoOwner": session.coordinates.owner,
        "repoName": session.coordinates.name,
        "number": number,
    })
}

// GHPRDetailsServiceImpl.loadDetails, with the base branch's rule the status builder reads.
fn load(
    check: Check,
    client: &Client,
    session: &Session,
    number: u64,
) -> Result<(PullRequestDetails, Option<RefUpdateRule>), HostingError> {
    check()?;
    let found = client
        .graphql_at(
            &session.coordinates.server.graphql_url(),
            Some(&session.token),
            Some(DETAILS_ACCEPT),
            DETAILS_QUERY,
            &variables(session, number),
            &["repository", "pullRequest"],
        )?
        .ok_or_else(|| HostingError::Io(format!("Pull request {number} does not exist")))?;
    let raw: RawDetails = serde_json::from_value(found).map_err(|_| parse_failed())?;
    let reviews = raw.reviews.into_items();
    let requests = raw.review_requests.into_items();
    let reviewers = reviewers::map(raw.author.as_ref(), &reviews, &requests);
    let rule = raw.base_ref.and_then(|base| base.ref_update_rule);
    let details = PullRequestDetails {
        number: raw.number,
        url: raw.url,
        title: raw.title,
        body: raw.body,
        state: raw.state,
        is_draft: raw.is_draft,
        base_ref_name: raw.base_ref_name,
        base_ref_oid: raw.base_ref_oid,
        head_ref_name: raw.head_ref_name,
        head_ref_oid: raw.head_ref_oid,
        head_repository: raw.head_repository.map(|repository| HeadRepository {
            owner: repository.owner.login,
            is_fork: repository.is_fork,
            name_with_owner: repository.name_with_owner,
            url: repository.url,
            ssh_url: repository.ssh_url,
        }),
        reviewers,
    };
    Ok((details, rule))
}

// `findPullRequest`.
pub fn details(
    check: Check,
    client: &Client,
    session: &Session,
    number: u64,
) -> Result<PullRequestDetails, HostingError> {
    load(check, client, session, number).map(|(details, _)| details)
}

// GHCommitStatusContextState.toCiState. A state GitHub adds later counts as pending.
fn context_state(state: &str) -> JobState {
    match state {
        "ERROR" | "EXPECTED" | "FAILURE" => JobState::Failed,
        "SUCCESS" => JobState::Success,
        _ => JobState::Pending,
    }
}

// GHCommitCheckSuiteConclusion?.toCiState: no conclusion yet is pending. A conclusion GitHub adds
// later counts as pending too.
fn run_state(conclusion: Option<&str>) -> JobState {
    match conclusion {
        Some(
            "ACTION_REQUIRED" | "CANCELLED" | "NEUTRAL" | "STALE" | "STARTUP_FAILURE" | "TIMED_OUT"
            | "FAILURE",
        ) => JobState::Failed,
        Some("SKIPPED") => JobState::Skipped,
        Some("SUCCESS") => JobState::Success,
        _ => JobState::Pending,
    }
}

// GHPRMergeabilityStateBuilder.build, with `withRestrictions` when the base branch has a rule.
fn build(
    head_ref_oid: String,
    mergeability: RawMergeability,
    rule: Option<&RefUpdateRule>,
) -> PullRequestStatus {
    let has_conflicts = match mergeability.mergeable.as_str() {
        "MERGEABLE" => Some(false),
        "CONFLICTING" => Some(true),
        _ => None,
    };
    let last = mergeability
        .commits
        .into_items()
        .pop()
        .map(|node| node.commit);
    let (contexts, suites) = match last {
        Some(commit) => (
            commit
                .status
                .map(|status| status.contexts)
                .unwrap_or_default(),
            commit.check_suites.into_items(),
        ),
        None => (Vec::new(), Vec::new()),
    };
    let required: Vec<&str> = rule
        .map(|rule| {
            rule.required_status_check_contexts
                .iter()
                .flatten()
                .map(String::as_str)
                .collect()
        })
        .unwrap_or_default();
    let passed: HashSet<&str> = contexts
        .iter()
        .filter(|context| context.state == "SUCCESS")
        .map(|context| context.context.as_str())
        .collect();
    let blocked = mergeability.merge_state_status == "BLOCKED";
    let blocking_failing =
        blocked && !required.is_empty() && !required.iter().all(|name| passed.contains(name));
    let is_restricted = rule.is_some_and(|rule| !rule.viewer_can_push);
    let count = rule
        .and_then(|rule| rule.required_approving_review_count)
        .unwrap_or(0);
    let required_approvals = if blocked && !is_restricted && !blocking_failing {
        count
    } else {
        0
    };
    let mut jobs: Vec<CiJob> = contexts
        .iter()
        .map(|context| CiJob {
            name: context.context.clone(),
            state: context_state(&context.state),
            is_required: context.is_required,
            url: context.target_url.clone(),
        })
        .collect();
    for run in suites
        .into_iter()
        .flat_map(|suite| suite.check_runs.into_items())
    {
        jobs.push(CiJob {
            name: run.name,
            state: run_state(run.conclusion.as_deref()),
            is_required: run.is_required,
            url: run.details_url.or(run.url),
        });
    }
    PullRequestStatus {
        head_ref_oid,
        jobs,
        has_conflicts,
        required_approvals,
        is_restricted,
    }
}

// GHPRStateServiceImpl.loadMergeabilityState: `findPullRequestMergeability`, built with the base
// branch's rule of `findPullRequest`, which 241 has loaded already and Ramify asks again.
pub fn status(
    check: Check,
    client: &Client,
    session: &Session,
    number: u64,
) -> Result<PullRequestStatus, HostingError> {
    let (details, rule) = load(check, client, session, number)?;
    check()?;
    let found = client
        .graphql_at(
            &session.coordinates.server.graphql_url(),
            Some(&session.token),
            Some(MERGEABILITY_ACCEPT),
            MERGEABILITY_QUERY,
            &variables(session, number),
            &["repository", "pullRequest"],
        )?
        .ok_or_else(|| HostingError::Io(format!("Could not find pull request {number}")))?;
    let raw: RawMergeability = serde_json::from_value(found).map_err(|_| parse_failed())?;
    Ok(build(details.head_ref_oid, raw, rule.as_ref()))
}

// `GHUser.getPresentableName` of `author.user ?: ghostUser`.
fn author_of(author: Option<RawGitActor>) -> String {
    author
        .and_then(|author| author.user)
        .and_then(|user| user.name.or(user.login))
        .unwrap_or_else(|| GHOST.to_string())
}

// buildCommitsTree and `depthFirstPostOrder`: the last commit is the last one in API order that is
// no other commit's parent, and the order is a depth-first post-order from it along the parents
// inside the pull request, oldest first. A commit it does not reach is left out, as in 241.
pub fn order(commits: Vec<PullRequestCommit>) -> Vec<PullRequestCommit> {
    let parents: HashSet<&str> = commits
        .iter()
        .flat_map(|commit| commit.parents.iter().map(String::as_str))
        .collect();
    let Some(last) = commits
        .iter()
        .rposition(|commit| !parents.contains(commit.oid.as_str()))
    else {
        return Vec::new();
    };
    let index: HashMap<&str, usize> = commits
        .iter()
        .enumerate()
        .map(|(at, commit)| (commit.oid.as_str(), at))
        .collect();
    let mut seen = vec![false; commits.len()];
    let mut order = Vec::with_capacity(commits.len());
    // Each entry is a commit and how many of its parents were visited.
    let mut stack = vec![(last, 0usize)];
    seen[last] = true;
    while let Some((at, next)) = stack.pop() {
        let parent = commits[at]
            .parents
            .get(next)
            .map(|parent| index.get(parent.as_str()).copied());
        match parent {
            Some(found) => {
                stack.push((at, next + 1));
                if let Some(parent) = found.filter(|&parent| !seen[parent]) {
                    seen[parent] = true;
                    stack.push((parent, 0));
                }
            }
            None => order.push(at),
        }
    }
    let mut slots: Vec<Option<PullRequestCommit>> = commits.into_iter().map(Some).collect();
    order
        .into_iter()
        .filter_map(|at| slots[at].take())
        .collect()
}

// `pullRequestCommits`, every page of 100, in P§6.5's order.
pub fn commits(
    check: Check,
    client: &Client,
    session: &Session,
    number: u64,
) -> Result<Vec<PullRequestCommit>, HostingError> {
    let mut variables = variables(session, number);
    variables["pageSize"] = json!(COMMITS_PAGE_SIZE);
    let nodes = client.graphql_pages(
        &session.coordinates.server.graphql_url(),
        Some(&session.token),
        None,
        COMMITS_QUERY,
        &variables,
        &["repository", "pullRequest", "commits"],
        check,
    )?;
    let mut commits = Vec::with_capacity(nodes.len());
    for node in nodes.into_iter().filter(|node| !node.is_null()) {
        let raw: RawCommitNode = serde_json::from_value(node).map_err(|_| parse_failed())?;
        let commit = raw.commit;
        commits.push(PullRequestCommit {
            oid: commit.oid,
            abbreviated_oid: commit.abbreviated_oid,
            headline: commit.message_headline,
            author: author_of(commit.author),
            committed_date: commit.committed_date,
            parents: commit
                .parents
                .into_items()
                .into_iter()
                .map(|parent| parent.oid)
                .collect(),
            url: commit.url,
        });
    }
    Ok(order(commits))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::hosting::mapping::RepoCoordinates;
    use crate::hosting::server::GithubServer;
    use crate::secrets::Secret;
    use crate::test_support::{FakeGitHub, Reply};

    const TOKEN: &str = "ghp_DETAILS0123456789";

    fn no_check() -> Result<(), HostingError> {
        Ok(())
    }

    fn session(fake: &FakeGitHub) -> Session {
        Session {
            coordinates: RepoCoordinates {
                server: GithubServer::parse(&fake.url()).unwrap(),
                owner: "o".to_string(),
                name: "r".to_string(),
            },
            token: Arc::new(Secret::new(TOKEN.to_string())),
        }
    }

    fn client() -> Client {
        Client::new(Duration::from_secs(5))
    }

    fn data(path: &[&str], node: Value) -> Reply {
        let mut body = node;
        for key in path.iter().rev() {
            body = json!({ *key: body });
        }
        Reply::json(
            "POST",
            "/api/graphql",
            200,
            "OK",
            &json!({ "data": body }).to_string(),
        )
    }

    fn user(id: &str, login: &str, name: Option<&str>) -> Value {
        json!({ "__typename": "User", "id": id, "login": login, "url": "", "avatarUrl": "", "name": name })
    }

    fn pull_request(extra: Value) -> Value {
        let mut node = json!({
            "id": "PR_7",
            "url": "https://github.com/o/r/pull/7",
            "number": 7,
            "title": "<img src=x onerror=alert(1)>",
            "state": "OPEN",
            "isDraft": false,
            "author": user("U_me", "me", None),
            "createdAt": "2026-11-09T10:00:00Z",
            "updatedAt": "2026-11-09T10:00:00Z",
            "isReadByViewer": true,
            "assignees": { "nodes": [] },
            "labels": { "nodes": [] },
            "reviewRequests": { "nodes": [] },
            "reviewThreads": { "nodes": [] },
            "reviews": { "nodes": [] },
            "reactions": { "pageInfo": {}, "nodes": [] },
            "mergeable": "MERGEABLE",
            "viewerCanUpdate": true,
            "viewerCanReact": true,
            "viewerDidAuthor": true,
            "body": "**bold** <script>x</script>\n\u{202e}rtl",
            "reviewDecision": null,
            "baseRef": { "refUpdateRule": null },
            "baseRefName": "main",
            "baseRefOid": "1".repeat(40),
            "baseRepository": null,
            "headRefName": "feature",
            "headRefOid": "2".repeat(40),
            "headRepository": {
                "id": "R_2",
                "owner": { "__typename": "User", "id": "U_f", "login": "forker" },
                "nameWithOwner": "forker/r",
                "viewerPermission": "READ",
                "mergeCommitAllowed": true,
                "squashMergeAllowed": true,
                "rebaseMergeAllowed": true,
                "defaultBranchRef": { "name": "main" },
                "isFork": true,
                "url": "https://github.com/forker/r",
                "sshUrl": "git@github.com:forker/r.git",
            },
        });
        for (key, value) in extra.as_object().unwrap() {
            node[key] = value.clone();
        }
        node
    }

    #[test]
    fn details_ask_find_pull_request_and_keep_its_texts_as_text() {
        let fake = FakeGitHub::new();
        let reviewed = json!({
            "reviews": { "nodes": [
                { "__typename": "PullRequestReview", "id": "RV_1", "url": "", "author": user("U_a", "alice", Some("Alice A")), "body": "", "state": "APPROVED", "createdAt": "", "viewerCanUpdate": false },
            ] },
            "reviewRequests": { "nodes": [
                { "requestedReviewer": user("U_b", "bob", None) },
            ] },
        });
        fake.reply(data(&["repository", "pullRequest"], pull_request(reviewed)));

        let found = details(&no_check, &client(), &session(&fake), 7).unwrap();

        assert_eq!(found.title, "<img src=x onerror=alert(1)>");
        assert_eq!(found.body, "**bold** <script>x</script>\n\u{202e}rtl");
        assert_eq!(found.base_ref_name, "main");
        assert_eq!(found.head_ref_oid, "2".repeat(40));
        assert_eq!(
            found.head_repository,
            Some(HeadRepository {
                owner: "forker".to_string(),
                is_fork: true,
                name_with_owner: "forker/r".to_string(),
                url: "https://github.com/forker/r".to_string(),
                ssh_url: "git@github.com:forker/r.git".to_string(),
            })
        );
        let names: Vec<(&str, reviewers::ReviewState)> = found
            .reviewers
            .iter()
            .map(|reviewer| (reviewer.name.as_str(), reviewer.state))
            .collect();
        assert_eq!(
            names,
            [
                ("Alice A", reviewers::ReviewState::Accepted),
                ("bob", reviewers::ReviewState::NeedReview),
            ]
        );
        let request = &fake.requests()[0];
        assert_eq!(request.header("Accept"), Some(DETAILS_ACCEPT));
        let sent: Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(sent["query"], DETAILS_QUERY);
        assert_eq!(
            sent["variables"],
            json!({ "repoOwner": "o", "repoName": "r", "number": 7 })
        );
        fake.finish();
    }

    #[test]
    fn a_missing_pull_request_says_241s_text() {
        let fake = FakeGitHub::new();
        fake.reply(data(&["repository", "pullRequest"], Value::Null));

        let error = details(&no_check, &client(), &session(&fake), 9).unwrap_err();

        assert_eq!(error.to_string(), "Pull request 9 does not exist");
        fake.finish();
    }

    fn context(name: &str, state: &str, required: bool) -> Value {
        json!({ "context": name, "state": state, "isRequired": required, "targetUrl": format!("https://github.com/o/r/runs/{name}") })
    }

    fn run(name: &str, conclusion: Option<&str>, required: bool) -> Value {
        json!({ "name": name, "conclusion": conclusion, "isRequired": required, "detailsUrl": null, "url": format!("https://github.com/o/r/runs/{name}") })
    }

    fn mergeability(
        status: &str,
        mergeable: &str,
        contexts: Vec<Value>,
        runs: Vec<Value>,
    ) -> RawMergeability {
        serde_json::from_value(json!({
            "mergeable": mergeable,
            "canBeRebased": true,
            "mergeStateStatus": status,
            "commits": { "pageInfo": {}, "nodes": [
                { "id": "C_old", "commit": { "id": "x", "oid": "x", "abbreviatedOid": "x", "status": { "contexts": [context("old", "FAILURE", true)] }, "checkSuites": { "pageInfo": {}, "nodes": [] } } },
                { "id": "C_last", "commit": {
                    "id": "y", "oid": "y", "abbreviatedOid": "y",
                    "status": { "contexts": contexts },
                    "checkSuites": { "pageInfo": {}, "nodes": [ { "checkRuns": { "pageInfo": {}, "nodes": runs } } ] },
                } },
            ] },
        }))
        .unwrap()
    }

    fn rule(count: Option<u32>, contexts: &[&str], can_push: bool) -> RefUpdateRule {
        RefUpdateRule {
            required_approving_review_count: count,
            required_status_check_contexts: contexts
                .iter()
                .map(|name| Some(name.to_string()))
                .collect(),
            viewer_can_push: can_push,
        }
    }

    #[test]
    fn the_jobs_are_the_last_commits_contexts_then_its_check_runs_with_241s_states() {
        let contexts = vec![
            context("error", "ERROR", false),
            context("expected", "EXPECTED", true),
            context("failure", "FAILURE", false),
            context("pending", "PENDING", false),
            context("success", "SUCCESS", true),
        ];
        // `detailsUrl ?: url`.
        let mut first = run("none", None, false);
        first["detailsUrl"] = json!("https://github.com/o/r/details");
        let runs = vec![
            first,
            run("action", Some("ACTION_REQUIRED"), false),
            run("cancelled", Some("CANCELLED"), false),
            run("neutral", Some("NEUTRAL"), false),
            run("stale", Some("STALE"), false),
            run("startup", Some("STARTUP_FAILURE"), false),
            run("timed", Some("TIMED_OUT"), false),
            run("failed", Some("FAILURE"), false),
            run("skipped", Some("SKIPPED"), false),
            run("passed", Some("SUCCESS"), true),
        ];
        let built = build(
            "h".to_string(),
            mergeability("CLEAN", "MERGEABLE", contexts, runs),
            None,
        );

        let states: Vec<(&str, JobState, bool)> = built
            .jobs
            .iter()
            .map(|job| (job.name.as_str(), job.state, job.is_required))
            .collect();
        use JobState::*;
        assert_eq!(
            states,
            [
                ("error", Failed, false),
                ("expected", Failed, true),
                ("failure", Failed, false),
                ("pending", Pending, false),
                ("success", Success, true),
                ("none", Pending, false),
                ("action", Failed, false),
                ("cancelled", Failed, false),
                ("neutral", Failed, false),
                ("stale", Failed, false),
                ("startup", Failed, false),
                ("timed", Failed, false),
                ("failed", Failed, false),
                ("skipped", Skipped, false),
                ("passed", Success, true),
            ]
        );
        assert_eq!(
            built.jobs[0].url.as_deref(),
            Some("https://github.com/o/r/runs/error")
        );
        assert_eq!(
            built.jobs[5].url.as_deref(),
            Some("https://github.com/o/r/details")
        );
        assert_eq!(built.has_conflicts, Some(false));
        assert_eq!(built.head_ref_oid, "h");
    }

    #[test]
    fn conflicts_are_known_only_when_github_knows() {
        let conflicts = |mergeable| {
            build(
                "h".into(),
                mergeability("DIRTY", mergeable, vec![], vec![]),
                None,
            )
            .has_conflicts
        };
        assert_eq!(conflicts("CONFLICTING"), Some(true));
        assert_eq!(conflicts("MERGEABLE"), Some(false));
        assert_eq!(conflicts("UNKNOWN"), None);
    }

    #[test]
    fn required_approvals_show_only_when_blocked_and_not_failing() {
        let count = |status: &str, contexts: Vec<Value>, rule: Option<RefUpdateRule>| {
            build(
                "h".into(),
                mergeability(status, "MERGEABLE", contexts, vec![]),
                rule.as_ref(),
            )
            .required_approvals
        };
        let passing = || vec![context("ci", "SUCCESS", true)];
        let failing = || vec![context("ci", "FAILURE", true)];
        // Blocked by the reviews alone: the count of the rule.
        assert_eq!(
            count("BLOCKED", passing(), Some(rule(Some(2), &["ci"], true))),
            2
        );
        assert_eq!(
            count("BLOCKED", failing(), Some(rule(Some(2), &[], true))),
            2
        );
        // Not blocked, a required check not passed, restricted, no count, or no rule: none.
        assert_eq!(
            count("CLEAN", passing(), Some(rule(Some(2), &["ci"], true))),
            0
        );
        assert_eq!(
            count("BEHIND", passing(), Some(rule(Some(2), &[], true))),
            0
        );
        assert_eq!(
            count("BLOCKED", failing(), Some(rule(Some(2), &["ci"], true))),
            0
        );
        assert_eq!(
            count("BLOCKED", vec![], Some(rule(Some(2), &["ci"], true))),
            0
        );
        assert_eq!(
            count("BLOCKED", passing(), Some(rule(Some(2), &["ci"], false))),
            0
        );
        assert_eq!(count("BLOCKED", passing(), Some(rule(None, &[], true))), 0);
        assert_eq!(count("BLOCKED", passing(), None), 0);
    }

    #[test]
    fn the_restriction_is_the_rules_viewer_can_push() {
        let restricted = |rule: Option<RefUpdateRule>| {
            build(
                "h".into(),
                mergeability("BLOCKED", "MERGEABLE", vec![], vec![]),
                rule.as_ref(),
            )
            .is_restricted
        };
        assert!(restricted(Some(rule(None, &[], false))));
        assert!(!restricted(Some(rule(None, &[], true))));
        assert!(!restricted(None));
    }

    #[test]
    fn status_asks_the_details_then_the_mergeability_with_their_previews() {
        let fake = FakeGitHub::new();
        let ruled = json!({ "baseRef": { "refUpdateRule": {
            "allowsDeletions": false, "allowsForcePushes": false, "pattern": "main",
            "requiredApprovingReviewCount": 1, "requiredStatusCheckContexts": [],
            "requiresLinearHistory": false, "requiresSignatures": false, "viewerCanPush": true,
        } } });
        let merge = json!({
            "mergeable": "CONFLICTING",
            "canBeRebased": false,
            "mergeStateStatus": "BLOCKED",
            "commits": { "pageInfo": {}, "nodes": [] },
        });
        fake.script(vec![
            data(&["repository", "pullRequest"], pull_request(ruled)),
            data(&["repository", "pullRequest"], merge),
        ]);

        let found = status(&no_check, &client(), &session(&fake), 7).unwrap();

        assert_eq!(
            found,
            PullRequestStatus {
                head_ref_oid: "2".repeat(40),
                jobs: vec![],
                has_conflicts: Some(true),
                required_approvals: 1,
                is_restricted: false,
            }
        );
        let requests = fake.requests();
        assert_eq!(requests[1].header("Accept"), Some(MERGEABILITY_ACCEPT));
        let sent: Value = serde_json::from_str(&requests[1].body).unwrap();
        assert_eq!(sent["query"], MERGEABILITY_QUERY);
        assert_eq!(
            sent["variables"],
            json!({ "repoOwner": "o", "repoName": "r", "number": 7 })
        );
        fake.finish();
    }

    fn commit(oid: &str, parents: &[&str]) -> PullRequestCommit {
        PullRequestCommit {
            oid: oid.to_string(),
            abbreviated_oid: oid.to_string(),
            headline: oid.to_string(),
            author: "a".to_string(),
            committed_date: String::new(),
            parents: parents.iter().map(|parent| parent.to_string()).collect(),
            url: String::new(),
        }
    }

    fn oids(commits: &[PullRequestCommit]) -> Vec<&str> {
        commits.iter().map(|commit| commit.oid.as_str()).collect()
    }

    #[test]
    fn commits_are_in_a_post_order_from_the_last_one() {
        // Linear, in API order or not, with the base outside: oldest first.
        let linear = vec![
            commit("b", &["a"]),
            commit("a", &["base"]),
            commit("c", &["b"]),
        ];
        assert_eq!(oids(&order(linear)), ["a", "b", "c"]);
        // A merge inside the pull request: the first parent's line first.
        let merged = vec![
            commit("a", &["base"]),
            commit("b", &["a"]),
            commit("x", &["base"]),
            commit("m", &["b", "x"]),
        ];
        assert_eq!(oids(&order(merged)), ["a", "b", "x", "m"]);
        // A merge of the base branch counts once, from its own parents.
        let synced = vec![
            commit("a", &["base"]),
            commit("m", &["a", "main2"]),
            commit("c", &["m"]),
        ];
        assert_eq!(oids(&order(synced)), ["a", "m", "c"]);
        // Two heads: the last one in API order; the other is not reached.
        let heads = vec![
            commit("a", &["base"]),
            commit("h1", &["a"]),
            commit("h2", &["a"]),
        ];
        assert_eq!(oids(&order(heads)), ["a", "h2"]);
        assert!(order(vec![]).is_empty());
    }

    fn commit_node(oid: &str, parents: &[&str], user: Value) -> Value {
        json!({
            "id": format!("PRC_{oid}"),
            "url": "",
            "commit": {
                "id": format!("C_{oid}"),
                "oid": oid,
                "abbreviatedOid": &oid[..1],
                "url": format!("https://github.com/o/r/commit/{oid}"),
                "messageHeadlineHTML": "<b>html</b>",
                "author": { "name": "Git Name", "email": "g@example.com", "avatarUrl": "", "date": "", "user": user },
                "committer": { "name": "Git Name", "email": "g@example.com", "avatarUrl": "", "date": "", "user": null },
                "messageHeadline": format!("Headline {oid} <i>"),
                "messageBodyHTML": "<p>html</p>",
                "committedDate": "2026-11-09T10:00:00Z",
                "parents": { "nodes": parents.iter().map(|parent| json!({ "id": "", "oid": parent, "abbreviatedOid": "" })).collect::<Vec<_>>() },
            },
        })
    }

    fn commits_page(nodes: Vec<Value>, end: Option<&str>, more: bool) -> Reply {
        data(
            &["repository", "pullRequest", "commits"],
            json!({ "pageInfo": { "startCursor": null, "hasPreviousPage": false, "endCursor": end, "hasNextPage": more }, "nodes": nodes }),
        )
    }

    #[test]
    fn commits_read_every_page_with_the_plain_headline_and_the_presentable_author() {
        let fake = FakeGitHub::new();
        fake.script(vec![
            commits_page(
                vec![commit_node(
                    "b",
                    &["a"],
                    user("U_1", "alice", Some("Alice A")),
                )],
                Some("CURSOR_1"),
                true,
            ),
            commits_page(
                vec![
                    commit_node("a", &["base"], user("U_2", "bob", None)),
                    commit_node("c", &["b"], Value::Null),
                ],
                None,
                false,
            ),
        ]);

        let found = commits(&no_check, &client(), &session(&fake), 7).unwrap();

        assert_eq!(oids(&found), ["a", "b", "c"]);
        let shown: Vec<(&str, &str)> = found
            .iter()
            .map(|commit| (commit.headline.as_str(), commit.author.as_str()))
            .collect();
        assert_eq!(
            shown,
            [
                ("Headline a <i>", "bob"),
                ("Headline b <i>", "Alice A"),
                ("Headline c <i>", "ghost")
            ]
        );
        assert_eq!(found[1].parents, ["a"]);
        assert_eq!(found[2].url, "https://github.com/o/r/commit/c");
        let requests = fake.requests();
        // No schema preview: only the client's default.
        assert!(!requests[0]
            .header("Accept")
            .unwrap_or_default()
            .contains("preview"));
        let first: Value = serde_json::from_str(&requests[0].body).unwrap();
        let second: Value = serde_json::from_str(&requests[1].body).unwrap();
        assert_eq!(first["query"], COMMITS_QUERY);
        assert_eq!(
            first["variables"],
            json!({ "repoOwner": "o", "repoName": "r", "number": 7, "pageSize": 100, "cursor": null })
        );
        assert_eq!(second["variables"]["cursor"], "CURSOR_1");
        fake.finish();
    }
}
