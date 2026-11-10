// The pull request list: its search, its pages, the update check and the filter choosers. Ports
// GHPRListSearchValue (toQuery), GHPRListLoader (buildQuery, pageSize), GHPRSearchQuery,
// GithubApiSearchQueryBuilder, GHPullRequestShort, GHPRListETagUpdateChecker, GithubApiRequests
// (Collaborators, Assignees, Labels, getListETag), GHPRRepositoryDataServiceImpl (the choosers),
// PersistingReviewListSearchHistoryModel and GHHtmlErrorPanel.getLoadingErrorText (plugins/github
// and platform/collaboration-tools at idea/241.14494.240). rama's own choice is decision 10 of the
// part 5 plan: the last search and the history are kept per repository.

use std::fs;
use std::io;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::storage;

use super::super::http::{Check, Client, HostingError, PARSE_FAILED};
use super::super::mapping::RepoCoordinates;
use super::reviewers::{self, RawActor, RawReview, RawReviewRequest, Reviewer};
use super::Session;

const SEARCH_FILE: &str = "github-pr-search.json";
const CORRUPT_SEARCH_FILE: &str = "github-pr-search.json.corrupt";
// GHGQLRequests.PullRequest.search, as 241 sends it (P Appendix A.1).
const QUERY: &str = include_str!("../graphql/issueSearch.graphql");
const SEARCH_ACCEPT: &str = "application/vnd.github.shadow-cat-preview+json";
// `Get.jsonPage`.
const REST_ACCEPT: &str = "application/vnd.github.v3+json";
// GHPRListLoader: `pageSize = 50`.
const PAGE_SIZE: u32 = 50;
// PersistingReviewListSearchHistoryModel: `historySizeLimit = 10`.
const HISTORY_LIMIT: usize = 10;
// GithubBundle 330 and 332.
const UNKNOWN_ERROR: &str = "Unknown loading error";
// GHHtmlErrorPanel, not localized in 241.
const REPOSITORY_EXPLANATION: &str = " Either repository doesn't exist or you don't have access. The most probable cause is that OAuth App access restrictions are enabled in organization.";

// Reading, changing and writing the search file is one step.
static SAVING: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StateFilter {
    Open,
    Closed,
    Merged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReviewFilter {
    NoReview,
    Required,
    Approved,
    ChangesRequested,
    ReviewedByMe,
    NotReviewedByMe,
    AwaitingReview,
}

// GHPRListSearchValue: the free text and the filters, each one `None` when not set. All `None`
// is `EMPTY`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchValue {
    pub text: Option<String>,
    pub state: Option<StateFilter>,
    pub assignee: Option<String>,
    pub review: Option<ReviewFilter>,
    pub author: Option<String>,
    pub label: Option<String>,
}

// GHPRListBuilder.buildQuery with GHPRListSearchValue.toQuery: `type:pr`, `repo:` with the API
// path, then the free text as it was typed, the state, the assignee, the review, the author and
// the label in double quotes. Terms are joined with one space, and a term whose value is empty is
// left out (`GithubApiSearchQueryBuilder.term`).
pub fn query_string(coordinates: &RepoCoordinates, value: &SearchValue) -> String {
    let mut terms: Vec<String> = vec![
        "type:pr".to_string(),
        format!("repo:{}/{}", coordinates.owner, coordinates.name),
    ];
    let mut term = |name: &str, value: &str| {
        if !value.is_empty() {
            terms.push(format!("{name}{value}"));
        }
    };
    if let Some(text) = &value.text {
        term("", text);
    }
    if let Some(state) = value.state {
        let state = match state {
            StateFilter::Open => "open",
            StateFilter::Closed => "closed",
            StateFilter::Merged => "merged",
        };
        term("is:", state);
    }
    if let Some(assignee) = &value.assignee {
        term("assignee:", assignee);
    }
    if let Some(review) = value.review {
        let (name, value) = match review {
            ReviewFilter::NoReview => ("review:", "none"),
            ReviewFilter::Required => ("review:", "required"),
            ReviewFilter::Approved => ("review:", "approved"),
            ReviewFilter::ChangesRequested => ("review:", "changes-requested"),
            ReviewFilter::ReviewedByMe => ("reviewed-by:", "@me"),
            ReviewFilter::NotReviewedByMe => ("-reviewed-by:", "@me"),
            ReviewFilter::AwaitingReview => ("review-requested:", "@me"),
        };
        term(name, value);
    }
    if let Some(author) = &value.author {
        term("author:", author);
    }
    if let Some(label) = &value.label {
        // `StringUtil.wrapWithDoubleQuote`: the quotes inside are not escaped.
        term("label:", &format!("\"{label}\""));
    }
    terms.join(" ")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Label {
    pub name: String,
    // Six hex digits without `#`, or nothing: any other colour GitHub sends is dropped.
    pub color: Option<String>,
}

// GHPullRequestShort, as far as the list row reads it. `author` is the login, `None` for a
// deleted account. `assignees` are presentable names. `unresolved` counts the review threads that
// are neither resolved nor outdated. `state` is `OPEN`, `CLOSED` or `MERGED`; `mergeable`
// `MERGEABLE`, `CONFLICTING` or `UNKNOWN`; `created_at` GitHub's ISO 8601 time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestShort {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub is_draft: bool,
    pub mergeable: String,
    pub author: Option<String>,
    pub created_at: String,
    pub labels: Vec<Label>,
    pub assignees: Vec<String>,
    pub reviewers: Vec<Reviewer>,
    pub unresolved: usize,
}

// One page of the search: `end_cursor` is the next page's `cursor`, while `has_next`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub items: Vec<PullRequestShort>,
    pub end_cursor: Option<String>,
    pub has_next: bool,
}

// GitHub's `nodes` may hold nulls.
#[derive(Deserialize)]
#[serde(bound = "T: Deserialize<'de>")]
pub(super) struct Nodes<T> {
    #[serde(default)]
    nodes: Vec<Option<T>>,
}

impl<T> Default for Nodes<T> {
    fn default() -> Self {
        Self { nodes: Vec::new() }
    }
}

impl<T> Nodes<T> {
    pub(super) fn into_items(self) -> Vec<T> {
        self.nodes.into_iter().flatten().collect()
    }
}

#[derive(Deserialize)]
struct RawLabel {
    name: String,
    color: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawThread {
    is_resolved: bool,
    is_outdated: bool,
}

// The `pullRequestInfoShort` fragment, as far as the row reads it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPullRequest {
    url: String,
    number: u64,
    title: String,
    state: String,
    is_draft: bool,
    author: Option<RawActor>,
    created_at: String,
    #[serde(default)]
    assignees: Nodes<RawActor>,
    #[serde(default)]
    labels: Nodes<RawLabel>,
    #[serde(default)]
    review_requests: Nodes<RawReviewRequest>,
    #[serde(default)]
    review_threads: Nodes<RawThread>,
    #[serde(default)]
    reviews: Nodes<RawReview>,
    mergeable: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPageInfo {
    end_cursor: Option<String>,
    #[serde(default)]
    has_next_page: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSearch {
    page_info: RawPageInfo,
    nodes: Vec<Option<Value>>,
}

fn parse_failed() -> HostingError {
    HostingError::Io(PARSE_FAILED.to_string())
}

// Label colours go into a style: only `rrggbb` is let through.
fn hex_color(color: Option<String>) -> Option<String> {
    color.filter(|color| color.len() == 6 && color.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn presentable(actor: &RawActor) -> Option<String> {
    actor.name.clone().or_else(|| actor.login.clone())
}

fn short(raw: RawPullRequest) -> PullRequestShort {
    let reviews = raw.reviews.into_items();
    let requests = raw.review_requests.into_items();
    let reviewers = reviewers::map(raw.author.as_ref(), &reviews, &requests);
    PullRequestShort {
        number: raw.number,
        title: raw.title,
        url: raw.url,
        state: raw.state,
        is_draft: raw.is_draft,
        mergeable: raw.mergeable,
        author: raw.author.and_then(|author| author.login),
        created_at: raw.created_at,
        labels: raw
            .labels
            .into_items()
            .into_iter()
            .map(|label| Label {
                name: label.name,
                color: hex_color(label.color),
            })
            .collect(),
        assignees: raw
            .assignees
            .into_items()
            .iter()
            .filter_map(presentable)
            .collect(),
        reviewers,
        unresolved: raw
            .review_threads
            .into_items()
            .iter()
            .filter(|thread| !thread.is_resolved && !thread.is_outdated)
            .count(),
    }
}

// One page of `issueSearch` (SimpleGHGQLPagesLoader with `pageSize = 50`): `cursor` is `None` for
// the first one. A node that is not a pull request is skipped.
pub fn page(
    check: Check,
    client: &Client,
    session: &Session,
    value: &SearchValue,
    cursor: Option<&str>,
) -> Result<Page, HostingError> {
    check()?;
    let variables = json!({
        "query": query_string(&session.coordinates, value),
        "pageSize": PAGE_SIZE,
        "cursor": cursor,
    });
    let found = client
        .graphql_at(
            &session.coordinates.server.graphql_url(),
            Some(&session.token),
            Some(SEARCH_ACCEPT),
            QUERY,
            &variables,
            &["search"],
        )?
        .ok_or_else(parse_failed)?;
    let search: RawSearch = serde_json::from_value(found).map_err(|_| parse_failed())?;
    let mut items = Vec::new();
    for node in search.nodes.into_iter().flatten() {
        if node.get("number").is_none() {
            continue;
        }
        let raw: RawPullRequest = serde_json::from_value(node).map_err(|_| parse_failed())?;
        items.push(short(raw));
    }
    Ok(Page {
        items,
        end_cursor: search.page_info.end_cursor,
        has_next: search.page_info.has_next_page,
    })
}

fn repository_url(session: &Session) -> String {
    let coordinates = &session.coordinates;
    format!(
        "{}/repos/{}/{}",
        coordinates.server.api_url(),
        coordinates.owner,
        coordinates.name
    )
}

// GHPRListETagUpdateChecker: the `ETag` of the newest open pull request, with no `Accept`.
pub fn etag(client: &Client, session: &Session) -> Result<Option<String>, HostingError> {
    let url = format!("{}/pulls?page=1&per_page=1", repository_url(session));
    client.etag(&url, Some(&session.token))
}

// The choosers of the `Author`, `Assignee` and `Label` filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChooserKind {
    Collaborators,
    Assignees,
    Labels,
}

// GithubApiPagesLoader over `Repos.Collaborators`, `Repos.Assignees` and `Repos.Labels`, which
// 241 asks without a query and pages by `Link`: the logins, or the label names. 241 builds the
// users without their full names (`GHUser(…, null)`), so the rows show the login alone.
pub fn choosers(
    check: Check,
    client: &Client,
    session: &Session,
    kind: ChooserKind,
) -> Result<Vec<String>, HostingError> {
    let (path, field) = match kind {
        ChooserKind::Collaborators => ("collaborators", "login"),
        ChooserKind::Assignees => ("assignees", "login"),
        ChooserKind::Labels => ("labels", "name"),
    };
    let url = format!("{}/{path}", repository_url(session));
    let items = client.pages_rest(&url, Some(&session.token), Some(REST_ACCEPT), check)?;
    items
        .iter()
        .map(|item| {
            item.get(field)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(parse_failed)
        })
        .collect()
}

// GHPRListPersistentSearchHistory: the last search (`None` until there is one, so the default
// applies) and the history, oldest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SavedSearch {
    pub last: Option<SearchValue>,
    pub history: Vec<SearchValue>,
}

pub fn saved_search(data: &Path) -> SavedSearch {
    storage::load_json(data, SEARCH_FILE, CORRUPT_SEARCH_FILE).unwrap_or_default()
}

// `lastFilter = value`, and with `remember` PersistingReviewListSearchHistoryModel.add: the value
// moves to the end, and only the last 10 stay. When to remember is the interface's call
// (ReviewListSearchPanelViewModelBase).
pub fn save_search(data: &Path, value: &SearchValue, remember: bool) -> io::Result<SavedSearch> {
    let _saving = SAVING.lock().unwrap_or_else(PoisonError::into_inner);
    let mut saved: SavedSearch = storage::load_json(data, SEARCH_FILE, CORRUPT_SEARCH_FILE)?;
    saved.last = Some(value.clone());
    if remember {
        saved.history.retain(|old| old != value);
        saved.history.push(value.clone());
        let extra = saved.history.len().saturating_sub(HISTORY_LIMIT);
        saved.history.drain(..extra);
    }
    fs::create_dir_all(data)?;
    let json = serde_json::to_vec_pretty(&saved).map_err(io::Error::other)?;
    storage::write_atomic(&data.join(SEARCH_FILE), &json)?;
    Ok(saved)
}

// GHHtmlErrorPanel.getLoadingErrorText, as plain text (a line break for `<br>`): for a status
// error with a JSON message, the message without its brackets, the explanation of a repository
// that cannot be resolved, then each error's message (or `{0} error in {1} field {2}`); else the
// error's own text.
pub fn loading_error_text(error: &HostingError) -> String {
    if let HostingError::Status {
        error: Some(json), ..
    } = error
    {
        if let Some(message) = &json.message {
            let message = message.strip_prefix('[').unwrap_or(message);
            let message = message.strip_suffix(']').unwrap_or(message);
            let mut text = message.to_string();
            if message
                .get(.."Could not resolve to a Repository".len())
                .is_some_and(|start| {
                    start.eq_ignore_ascii_case("Could not resolve to a Repository")
                })
            {
                text.push_str(REPOSITORY_EXPLANATION);
            }
            let details: Vec<String> = json
                .errors
                .iter()
                .flatten()
                .map(|detail| {
                    detail.message.clone().unwrap_or_else(|| {
                        let part = |value: &Option<String>| {
                            value.clone().unwrap_or_else(|| "null".to_string())
                        };
                        format!(
                            "{} error in {} field {}",
                            part(&detail.code),
                            part(&detail.resource),
                            detail.field.clone().unwrap_or_default()
                        )
                    })
                })
                .collect();
            if !details.is_empty() {
                text.push_str(":\n");
                text.push_str(&details.join("\n"));
            }
            return text;
        }
    }
    let text = error.to_string();
    if text.is_empty() {
        UNKNOWN_ERROR.to_string()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;
    use tempfile::TempDir;

    use super::*;
    use crate::hosting::http::{ErrorDetail, ErrorMessage};
    use crate::hosting::server::GithubServer;
    use crate::secrets::Secret;
    use crate::test_support::{FakeGitHub, Reply};

    const TOKEN: &str = "ghp_LIST0123456789";

    fn no_check() -> Result<(), HostingError> {
        Ok(())
    }

    fn coordinates(server: &str, owner: &str, name: &str) -> RepoCoordinates {
        RepoCoordinates {
            server: GithubServer::parse(server).unwrap(),
            owner: owner.to_string(),
            name: name.to_string(),
        }
    }

    fn session(fake: &FakeGitHub) -> Session {
        Session {
            coordinates: coordinates(&fake.url(), "new-owner", "new-name"),
            token: std::sync::Arc::new(Secret::new(TOKEN.to_string())),
        }
    }

    fn client() -> Client {
        Client::new(Duration::from_secs(5))
    }

    fn some(text: &str) -> Option<String> {
        Some(text.to_string())
    }

    #[test]
    fn the_search_string_is_241s() {
        let repo = coordinates("github.com", "o", "r");
        assert_eq!(
            query_string(&repo, &SearchValue::default()),
            "type:pr repo:o/r"
        );
        let every = SearchValue {
            text: some("fix  \"crash\" is:closed"),
            state: Some(StateFilter::Merged),
            assignee: some("alice"),
            review: Some(ReviewFilter::NotReviewedByMe),
            author: some("bob"),
            label: some("good first issue"),
        };
        assert_eq!(
            query_string(&repo, &every),
            "type:pr repo:o/r fix  \"crash\" is:closed is:merged assignee:alice \
             -reviewed-by:@me author:bob label:\"good first issue\""
        );
        let review = |review| SearchValue {
            review: Some(review),
            ..SearchValue::default()
        };
        let terms = [
            (ReviewFilter::NoReview, "review:none"),
            (ReviewFilter::Required, "review:required"),
            (ReviewFilter::Approved, "review:approved"),
            (ReviewFilter::ChangesRequested, "review:changes-requested"),
            (ReviewFilter::ReviewedByMe, "reviewed-by:@me"),
            (ReviewFilter::NotReviewedByMe, "-reviewed-by:@me"),
            (ReviewFilter::AwaitingReview, "review-requested:@me"),
        ];
        for (filter, term) in terms {
            assert_eq!(
                query_string(&repo, &review(filter)),
                format!("type:pr repo:o/r {term}")
            );
        }
        let state = |state| SearchValue {
            state: Some(state),
            ..SearchValue::default()
        };
        assert_eq!(
            query_string(&repo, &state(StateFilter::Open)),
            "type:pr repo:o/r is:open"
        );
        assert_eq!(
            query_string(&repo, &state(StateFilter::Closed)),
            "type:pr repo:o/r is:closed"
        );
        // An empty value is no term; the label's quotes make it one, and quotes inside stay.
        let empty = SearchValue {
            text: some(""),
            assignee: some(""),
            author: some(""),
            label: some("a\"b"),
            ..SearchValue::default()
        };
        assert_eq!(
            query_string(&repo, &empty),
            "type:pr repo:o/r label:\"a\"b\""
        );
    }

    fn pull_request(number: u64, extra: Value) -> Value {
        let mut node = json!({
            "id": format!("PR_{number}"),
            "url": format!("https://github.com/o/r/pull/{number}"),
            "number": number,
            "title": format!("Pull request {number}"),
            "state": "OPEN",
            "isDraft": false,
            "author": { "__typename": "User", "id": "U_me", "login": "me", "url": "", "avatarUrl": "", "name": "Me Myself" },
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
        });
        for (key, value) in extra.as_object().unwrap() {
            node[key] = value.clone();
        }
        node
    }

    fn search(nodes: Vec<Value>, end: Option<&str>, more: bool) -> Reply {
        let body = json!({
            "data": {
                "search": {
                    "pageInfo": { "startCursor": null, "hasPreviousPage": false, "endCursor": end, "hasNextPage": more },
                    "nodes": nodes,
                }
            }
        });
        Reply::json("POST", "/api/graphql", 200, "OK", &body.to_string())
    }

    #[test]
    fn a_page_asks_issue_search_as_241_does() {
        let fake = FakeGitHub::new();
        let session = session(&fake);
        fake.script(vec![
            search(vec![pull_request(7, json!({}))], Some("CURSOR_1"), true),
            search(vec![], None, false),
        ]);
        let value = SearchValue {
            state: Some(StateFilter::Open),
            assignee: some("me"),
            ..SearchValue::default()
        };
        let first = page(&no_check, &client(), &session, &value, None).unwrap();
        assert_eq!(first.end_cursor.as_deref(), Some("CURSOR_1"));
        assert!(first.has_next);
        assert_eq!(first.items.len(), 1);
        let second = page(&no_check, &client(), &session, &value, Some("CURSOR_1")).unwrap();
        assert_eq!(
            second,
            Page {
                items: vec![],
                end_cursor: None,
                has_next: false
            }
        );

        let requests = fake.requests();
        let body: Value = serde_json::from_str(&requests[0].body).unwrap();
        assert_eq!(body["query"], QUERY);
        assert_eq!(
            body["variables"],
            json!({
                "query": "type:pr repo:new-owner/new-name is:open assignee:me",
                "pageSize": 50,
                "cursor": null,
            })
        );
        let body: Value = serde_json::from_str(&requests[1].body).unwrap();
        assert_eq!(body["variables"]["cursor"], "CURSOR_1");
        assert_eq!(
            requests[0].header("accept"),
            Some("application/vnd.github.shadow-cat-preview+json")
        );
        assert_eq!(
            requests[0].header("authorization"),
            Some(format!("Bearer {TOKEN}").as_str())
        );
        fake.finish();
    }

    #[test]
    fn a_row_reads_what_241_shows() {
        let fake = FakeGitHub::new();
        let session = session(&fake);
        let node = pull_request(
            12,
            json!({
                "state": "CLOSED",
                "isDraft": true,
                "mergeable": "CONFLICTING",
                "author": null,
                "assignees": { "nodes": [
                    { "__typename": "User", "id": "U_a", "login": "alice", "name": "Alice A" },
                    null,
                    { "__typename": "User", "id": "U_b", "login": "bob", "name": null },
                ] },
                "reviewThreads": { "nodes": [
                    { "isResolved": false, "isOutdated": false },
                    { "isResolved": true, "isOutdated": false },
                    { "isResolved": false, "isOutdated": true },
                    { "isResolved": false, "isOutdated": false },
                ] },
                "reviews": { "nodes": [
                    { "__typename": "PullRequestReview", "id": "R_1", "url": "", "body": "", "state": "APPROVED",
                      "author": { "__typename": "User", "id": "U_c", "login": "carol", "name": null },
                      "createdAt": "", "viewerCanUpdate": false },
                ] },
                "reviewRequests": { "nodes": [
                    { "requestedReviewer": { "__typename": "Team", "id": "T_1", "slug": "core", "combinedSlug": "o/core", "name": "Core" } },
                ] },
            }),
        );
        // An issue the search let through has none of the fields: it is skipped.
        fake.reply(search(vec![json!({}), node, Value::Null], None, false));
        let found = page(
            &no_check,
            &client(),
            &session,
            &SearchValue::default(),
            None,
        )
        .unwrap();
        assert_eq!(
            found.items,
            [PullRequestShort {
                number: 12,
                title: "Pull request 12".to_string(),
                url: "https://github.com/o/r/pull/12".to_string(),
                state: "CLOSED".to_string(),
                is_draft: true,
                mergeable: "CONFLICTING".to_string(),
                author: None,
                created_at: "2026-11-09T10:00:00Z".to_string(),
                labels: vec![],
                assignees: vec!["Alice A".to_string(), "bob".to_string()],
                reviewers: vec![
                    Reviewer {
                        name: "carol".to_string(),
                        state: reviewers::ReviewState::Accepted,
                    },
                    Reviewer {
                        name: "Core".to_string(),
                        state: reviewers::ReviewState::NeedReview,
                    },
                ],
                unresolved: 2,
            }]
        );
        fake.finish();
    }

    #[test]
    fn a_title_with_html_is_text_and_a_bad_label_colour_is_dropped() {
        let fake = FakeGitHub::new();
        let session = session(&fake);
        let title = "<img src=x onerror=alert(1)>\u{202e}\u{7}";
        let node = pull_request(
            3,
            json!({
                "title": title,
                "labels": { "nodes": [
                    { "id": "L_1", "url": "", "name": "bug", "color": "d73a4a" },
                    { "id": "L_2", "url": "", "name": "<b>x</b>", "color": "red;background:url(x)" },
                    { "id": "L_3", "url": "", "name": "short", "color": "fff" },
                    { "id": "L_4", "url": "", "name": "none", "color": null },
                    { "id": "L_5", "url": "", "name": "hash", "color": "#d73a4a" },
                ] },
            }),
        );
        fake.reply(search(vec![node], None, false));
        let found = page(
            &no_check,
            &client(),
            &session,
            &SearchValue::default(),
            None,
        )
        .unwrap();
        let row = &found.items[0];
        // The title comes back as it was sent; the interface shows it as text.
        assert_eq!(row.title, title);
        let label = |name: &str, color: Option<&str>| Label {
            name: name.to_string(),
            color: color.map(str::to_string),
        };
        assert_eq!(
            row.labels,
            [
                label("bug", Some("d73a4a")),
                label("<b>x</b>", None),
                label("short", None),
                label("none", None),
                label("hash", None),
            ]
        );
        fake.finish();
    }

    #[test]
    fn a_page_that_is_not_a_search_fails_with_241s_texts() {
        let fake = FakeGitHub::new();
        let session = session(&fake);
        fake.script(vec![
            Reply::json(
                "POST",
                "/api/graphql",
                200,
                "OK",
                r#"{"data":{"search":{"pageInfo":{},"nodes":[{"number":"x"}]}}}"#,
            ),
            Reply::json(
                "POST",
                "/api/graphql",
                401,
                "Unauthorized",
                r#"{"message":"Bad credentials"}"#,
            ),
        ]);
        let value = SearchValue::default();
        assert_eq!(
            page(&no_check, &client(), &session, &value, None).unwrap_err(),
            HostingError::Io(PARSE_FAILED.to_string())
        );
        let error = page(&no_check, &client(), &session, &value, None).unwrap_err();
        assert!(error.is_authentication());
        // A cancelled page asks nothing.
        let cancelled = || Err(HostingError::Cancelled);
        assert_eq!(
            page(&cancelled, &client(), &session, &value, None).unwrap_err(),
            HostingError::Cancelled
        );
        assert_eq!(fake.requests().len(), 2);
        fake.finish();
    }

    #[test]
    fn the_update_check_reads_the_etag_of_the_newest_open_pull_request() {
        let fake = FakeGitHub::new();
        let session = session(&fake);
        fake.script(vec![
            Reply::new(
                "GET",
                "/api/v3/repos/new-owner/new-name/pulls?page=1&per_page=1",
                200,
                "OK",
                &[("ETag", "W/\"abc\""), ("Content-Type", "application/json")],
                "[]",
            ),
            Reply::new(
                "GET",
                "/api/v3/repos/new-owner/new-name/pulls?page=1&per_page=1",
                200,
                "OK",
                &[("Content-Type", "application/json")],
                "[]",
            ),
        ]);
        assert_eq!(
            etag(&client(), &session).unwrap().as_deref(),
            Some("W/\"abc\"")
        );
        assert_eq!(etag(&client(), &session).unwrap(), None);
        let requests = fake.requests();
        // The request sets no `Accept`, as in 241: what arrives is the HTTP client's default.
        assert_eq!(requests[0].header("accept"), Some("*/*"));
        assert_eq!(
            requests[0].header("authorization"),
            Some(format!("Bearer {TOKEN}").as_str())
        );
        fake.finish();
    }

    #[test]
    fn the_choosers_read_every_rest_page() {
        let fake = FakeGitHub::new();
        let session = session(&fake);
        let next = format!(
            "<{}/api/v3/repos/new-owner/new-name/collaborators?page=2>; rel=\"next\"",
            fake.url()
        );
        fake.script(vec![
            Reply::new(
                "GET",
                "/api/v3/repos/new-owner/new-name/collaborators",
                200,
                "OK",
                &[("Content-Type", "application/json"), ("Link", &next)],
                r#"[{"login":"alice"},{"login":"bob"}]"#,
            ),
            Reply::json(
                "GET",
                "/api/v3/repos/new-owner/new-name/collaborators?page=2",
                200,
                "OK",
                r#"[{"login":"carol"}]"#,
            ),
            Reply::json(
                "GET",
                "/api/v3/repos/new-owner/new-name/assignees",
                200,
                "OK",
                r#"[{"login":"dave"}]"#,
            ),
            Reply::json(
                "GET",
                "/api/v3/repos/new-owner/new-name/labels",
                200,
                "OK",
                r#"[{"name":"bug","color":"d73a4a"},{"name":"good first issue"}]"#,
            ),
            Reply::json(
                "GET",
                "/api/v3/repos/new-owner/new-name/labels",
                404,
                "Not Found",
                r#"{"message":"Not Found"}"#,
            ),
        ]);
        let load = |kind| choosers(&no_check, &client(), &session, kind);
        assert_eq!(
            load(ChooserKind::Collaborators).unwrap(),
            ["alice", "bob", "carol"]
        );
        assert_eq!(load(ChooserKind::Assignees).unwrap(), ["dave"]);
        assert_eq!(
            load(ChooserKind::Labels).unwrap(),
            ["bug", "good first issue"]
        );
        assert_eq!(
            load(ChooserKind::Labels).unwrap_err().to_string(),
            "404 Not Found - Not Found"
        );
        let requests = fake.requests();
        assert_eq!(
            requests[0].header("accept"),
            Some("application/vnd.github.v3+json")
        );
        fake.finish();
    }

    #[test]
    fn the_last_search_and_ten_history_entries_are_kept_per_repository() {
        let data = TempDir::new().unwrap();
        assert_eq!(saved_search(data.path()), SavedSearch::default());
        let text = |text: &str| SearchValue {
            text: some(text),
            ..SearchValue::default()
        };
        // Saving without remembering changes only the last search.
        let saved = save_search(data.path(), &text("a"), false).unwrap();
        assert_eq!(saved.last, Some(text("a")));
        assert!(saved.history.is_empty());
        for index in 0..12 {
            save_search(data.path(), &text(&index.to_string()), true).unwrap();
        }
        // The same search again moves to the end.
        let saved = save_search(data.path(), &text("5"), true).unwrap();
        let history: Vec<String> = saved
            .history
            .iter()
            .map(|value| value.text.clone().unwrap())
            .collect();
        assert_eq!(
            history,
            ["2", "3", "4", "6", "7", "8", "9", "10", "11", "5"]
        );
        assert_eq!(saved_search(data.path()), saved);
        // The empty search is a search: it is restored, not replaced by the default.
        save_search(data.path(), &SearchValue::default(), false).unwrap();
        assert_eq!(saved_search(data.path()).last, Some(SearchValue::default()));
        // A damaged file is no search.
        fs::write(data.path().join(SEARCH_FILE), "{").unwrap();
        assert_eq!(saved_search(data.path()), SavedSearch::default());
    }

    #[test]
    fn errors_show_cant_load_with_re_login_for_auth_and_retry_otherwise() {
        // The texts under `Can't load pull requests` (the interface puts `User authorization
        // failed` for an authentication error).
        let status =
            |message: Option<&str>, errors: Option<Vec<ErrorDetail>>| HostingError::Status {
                code: 404,
                reason: "Not Found".to_string(),
                message: "raw".to_string(),
                error: Some(ErrorMessage {
                    message: message.map(str::to_string),
                    errors,
                }),
            };
        assert_eq!(
            loading_error_text(&status(Some("[Not Found]"), None)),
            "Not Found"
        );
        assert_eq!(
            loading_error_text(&status(
                Some("could not resolve to a Repository with the name 'o/r'."),
                None
            )),
            format!(
                "could not resolve to a Repository with the name 'o/r'.{REPOSITORY_EXPLANATION}"
            )
        );
        let detail = |message: Option<&str>| ErrorDetail {
            resource: Some("Issue".to_string()),
            field: Some("title".to_string()),
            code: Some("missing_field".to_string()),
            message: message.map(str::to_string),
        };
        assert_eq!(
            loading_error_text(&status(
                Some("Validation Failed"),
                Some(vec![detail(Some("Title is too long")), detail(None)])
            )),
            "Validation Failed:\nTitle is too long\nmissing_field error in Issue field title"
        );
        // Without a JSON message: the error's own text.
        assert_eq!(
            loading_error_text(&status(None, None)),
            "404 Not Found - raw"
        );
        assert_eq!(
            loading_error_text(&HostingError::Unreachable),
            "Server is unreachable"
        );
        assert_eq!(
            loading_error_text(&HostingError::GraphQl("[a, b]".to_string())),
            "[a, b]"
        );
        assert_eq!(
            loading_error_text(&HostingError::Io(String::new())),
            "Unknown loading error"
        );
    }
}
