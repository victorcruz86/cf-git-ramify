// The HTTP client for GitHub's REST and GraphQL APIs. It follows GithubApiRequestExecutor,
// GithubApiRequest, GithubErrorMessage, GHEServerVersionChecker and LinkHttpHeaderValue (plugins/
// github and platform/collaboration-tools at idea/241.14494.240), and HttpRequests for the
// redirects. rama's own choices (decision 12 of the part 5 plan): the user agent is `rama`, there
// is no gzip, and the `Authorization` header only goes to the scheme, host and port the caller
// named.

use std::fmt;
use std::time::Duration;

use serde_json::Value;

use crate::secrets::Secret;

pub const USER_AGENT: &str = "rama";
// Connections wait for the timeout of the settings; reads for HttpRequests' default.
pub const READ_TIMEOUT: Duration = Duration::from_secs(60);

// The texts are 241's. `Authentication` holds GithubErrorMessage's presentable error, and
// `Status` the presentable error or the raw body, plus the JSON error itself when there was one
// (GHHtmlErrorPanel.getLoadingErrorText reads its parts). `Io` is everything else 241 shows by
// its message: a refused connection, a timeout, a body that is not JSON, an unsupported GitHub
// Enterprise. `Cancelled` is the caller's check between pages. None of them carries a request
// header, the request's URL or the token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostingError {
    Authentication(String),
    RateLimit(String),
    Status {
        code: u16,
        reason: String,
        message: String,
        error: Option<ErrorMessage>,
    },
    GraphQl(String),
    Scopes,
    Unreachable,
    Io(String),
    // The account has no token in the store.
    MissingToken,
    Cancelled,
}

impl HostingError {
    // What 241 throws as a GithubAuthenticationException: the places that offer `Log in` or
    // `Re-Login` ask this.
    pub fn is_authentication(&self) -> bool {
        matches!(self, HostingError::Authentication(_) | HostingError::Scopes)
    }
}

impl fmt::Display for HostingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // GithubBundle 344.
            HostingError::Authentication(message) => {
                write!(formatter, "Request response: {message}.")
            }
            HostingError::RateLimit(message)
            | HostingError::GraphQl(message)
            | HostingError::Io(message) => formatter.write_str(message),
            HostingError::Status {
                code,
                reason,
                message,
                ..
            } => write!(formatter, "{code} {reason} - {message}"),
            HostingError::Scopes => {
                formatter.write_str("Access token has not been granted the required scopes.")
            }
            // GithubBundle 331.
            HostingError::Unreachable => formatter.write_str("Server is unreachable"),
            // CollaborationToolsBundle 8.
            HostingError::MissingToken => formatter.write_str("Missing access token"),
            // As every other cancelled operation of rama (`GitError::Cancelled`).
            HostingError::Cancelled => formatter.write_str("The operation was cancelled"),
        }
    }
}

pub(crate) const PARSE_FAILED: &str = "Can't parse GitHub response";
const UNKNOWN_ERROR: &str = "Unknown loading error";
const TOO_MANY_REDIRECTS: &str = "Connection failed: too many redirects";
const NOT_NULLABLE: &str = "Non-nullable entity is null or entity path is invalid";
const UNDEFINED: &str = "Undefined request state - both result and errors are null";
const ENTERPRISE_VERSION: &str = "x-github-enterprise-version";
// HttpRequests.REDIRECT_LIMIT and REDIRECTS.
const REDIRECT_LIMIT: usize = 10;
const REDIRECTS: [u16; 5] = [302, 307, 301, 303, 308];

// A response below 400, read completely.
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        header(&self.headers, name)
    }

    pub fn json(&self) -> Result<Value, HostingError> {
        serde_json::from_str(&self.body).map_err(|_| HostingError::Io(PARSE_FAILED.to_string()))
    }
}

// Whatever the server answered, before the status is judged.
struct Answer {
    status: u16,
    reason: String,
    headers: Vec<(String, String)>,
    body: String,
}

pub struct Client {
    agent: ureq::Agent,
    // Why `https://` cannot be used, when the system's TLS could not be set up.
    tls_failure: Option<String>,
}

// What the caller checks between pages: `ctx.check_cancelled` of its task, mapped to
// `HostingError::Cancelled`.
pub type Check<'a> = &'a dyn Fn() -> Result<(), HostingError>;

impl Client {
    pub fn new(connect_timeout: Duration) -> Self {
        Self::with_timeouts(connect_timeout, READ_TIMEOUT)
    }

    pub(crate) fn with_timeouts(connect: Duration, read: Duration) -> Self {
        let connector = native_tls::TlsConnector::new().map_err(|error| error.to_string());
        Self::with_connector(connect, read, connector)
    }

    fn with_connector(
        connect: Duration,
        read: Duration,
        connector: Result<native_tls::TlsConnector, String>,
    ) -> Self {
        Self::from_builder(Self::builder(connect, read), connector)
    }

    // For the tests: every host name goes through `resolver`, so a test can say where a name
    // leads, or that it leads nowhere, without asking the system's DNS.
    #[cfg(test)]
    pub(crate) fn resolving(connect: Duration, resolver: impl ureq::Resolver + 'static) -> Self {
        let connector = native_tls::TlsConnector::new().map_err(|error| error.to_string());
        Self::from_builder(
            Self::builder(connect, READ_TIMEOUT).resolver(resolver),
            connector,
        )
    }

    // A zero connect timeout is none, as `URLConnection.setConnectTimeout(0)`. ureq takes zero for
    // a deadline already passed, and leaving it unset gives its own 30 s; `Duration::MAX` overflows
    // the deadline, which ureq reads as no deadline.
    fn builder(connect: Duration, read: Duration) -> ureq::AgentBuilder {
        let connect = if connect.is_zero() {
            Duration::MAX
        } else {
            connect
        };
        ureq::AgentBuilder::new()
            .timeout_connect(connect)
            .timeout_read(read)
            .user_agent(USER_AGENT)
            // Redirects are followed here, by hand, to decide where the token goes.
            .redirects(0)
    }

    fn from_builder(
        mut builder: ureq::AgentBuilder,
        connector: Result<native_tls::TlsConnector, String>,
    ) -> Self {
        let tls_failure = match connector {
            Ok(connector) => {
                builder = builder.tls_connector(std::sync::Arc::new(connector));
                None
            }
            Err(error) => Some(error),
        };
        Self {
            agent: builder.build(),
            tls_failure,
        }
    }

    pub fn get(
        &self,
        url: &str,
        token: Option<&Secret>,
        accept: Option<&str>,
    ) -> Result<Response, HostingError> {
        self.get_for(url, url, token, accept)
    }

    // GHPRListETagUpdateChecker: no `Accept`, only the `ETag` of the answer.
    pub fn etag(&self, url: &str, token: Option<&Secret>) -> Result<Option<String>, HostingError> {
        let response = self.get(url, token, None)?;
        Ok(response.header("etag").map(str::to_string))
    }

    // GithubApiPagesLoader with JsonPage: every page's array, following `Link: rel="next"`.
    // `check` runs before each page.
    pub fn pages_rest(
        &self,
        url: &str,
        token: Option<&Secret>,
        accept: Option<&str>,
        check: Check,
    ) -> Result<Vec<Value>, HostingError> {
        let mut items = Vec::new();
        let mut next = Some(url.to_string());
        while let Some(page) = next {
            check()?;
            let response = self.get_for(url, &page, token, accept)?;
            match response.json()? {
                Value::Array(values) => items.extend(values),
                _ => return Err(HostingError::Io(PARSE_FAILED.to_string())),
            }
            next = match response.header("link") {
                Some(link) => next_link(link).map_err(|error| hidden(error, token))?,
                None => None,
            };
        }
        Ok(items)
    }

    // GQLQuery.Parsed: `data` when it is there, else the errors.
    pub fn graphql(
        &self,
        url: &str,
        token: Option<&Secret>,
        accept: Option<&str>,
        query: &str,
        variables: &Value,
    ) -> Result<Value, HostingError> {
        let (data, errors) = self.post_query(url, token, accept, query, variables)?;
        match data {
            Some(data) => Ok(data),
            None => Err(match errors {
                Some(errors) => graphql_failure(&errors, token),
                None => HostingError::Io(UNDEFINED.to_string()),
            }),
        }
    }

    // GQLQuery.parseResponse (OptionalTraversedParsed): what is at `path` under `data`; else the
    // errors, or `None` when there are none. 241 stops at a missing key and returns the parent;
    // here a missing key is a null.
    pub fn graphql_at(
        &self,
        url: &str,
        token: Option<&Secret>,
        accept: Option<&str>,
        query: &str,
        variables: &Value,
        path: &[&str],
    ) -> Result<Option<Value>, HostingError> {
        let (data, errors) = self.post_query(url, token, accept, query, variables)?;
        let found = data.and_then(|data| {
            path.iter()
                .try_fold(data, |node, key| node.get(key).cloned())
                .filter(|node| !node.is_null())
        });
        match (found, errors) {
            (Some(node), _) => Ok(Some(node)),
            (None, Some(errors)) => Err(graphql_failure(&errors, token)),
            (None, None) => Ok(None),
        }
    }

    // GHGQLPagesLoader: the `nodes` of the connection at `path`, page after page, with the
    // variable `cursor` set to the previous page's `endCursor` (null on the first) while
    // `pageInfo.hasNextPage`. `check` runs before each page.
    #[allow(clippy::too_many_arguments)]
    pub fn graphql_pages(
        &self,
        url: &str,
        token: Option<&Secret>,
        accept: Option<&str>,
        query: &str,
        variables: &Value,
        path: &[&str],
        check: Check,
    ) -> Result<Vec<Value>, HostingError> {
        let parse_failed = || HostingError::Io(PARSE_FAILED.to_string());
        let mut variables = variables.as_object().cloned().ok_or_else(parse_failed)?;
        let mut cursor = Value::Null;
        let mut nodes = Vec::new();
        loop {
            check()?;
            variables.insert("cursor".to_string(), cursor);
            let page = self
                .graphql_at(
                    url,
                    token,
                    accept,
                    query,
                    &Value::Object(variables.clone()),
                    path,
                )?
                .ok_or_else(|| HostingError::Io(NOT_NULLABLE.to_string()))?;
            let items = page
                .get("nodes")
                .and_then(Value::as_array)
                .ok_or_else(parse_failed)?;
            nodes.extend(items.iter().cloned());
            let info = page.get("pageInfo");
            let more = info
                .and_then(|info| info.get("hasNextPage"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let end = info
                .and_then(|info| info.get("endCursor"))
                .and_then(Value::as_str);
            match (more, end) {
                (true, Some(end)) => cursor = Value::String(end.to_string()),
                _ => return Ok(nodes),
            }
        }
    }

    // HttpRequests.openConnection's loop: up to 10 requests, following 301, 302, 303, 307 and
    // 308 with a `Location`. Then GithubApiRequestExecutor's checks of the status and of the
    // Enterprise version. The token goes only to the scheme, host and port of `origin`: a
    // redirect or a `Link` to another server gets none.
    fn get_for(
        &self,
        origin: &str,
        url: &str,
        token: Option<&Secret>,
        accept: Option<&str>,
    ) -> Result<Response, HostingError> {
        let mut current = url.to_string();
        for _ in 0..REDIRECT_LIMIT {
            let answer = self.exchange("GET", origin, &current, token, accept, None)?;
            if REDIRECTS.contains(&answer.status) {
                if let Some(location) = header(&answer.headers, "location") {
                    current = resolve(&current, location)?;
                    continue;
                }
            }
            return checked(answer, token);
        }
        Err(HostingError::Io(TOO_MANY_REDIRECTS.to_string()))
    }

    // A GraphQL POST (GithubApiRequest.Post with GQLQuery's body). As in HttpRequests, a request
    // with a body follows no redirect. Returns `data` when not null, and `errors`.
    #[allow(clippy::type_complexity)]
    fn post_query(
        &self,
        url: &str,
        token: Option<&Secret>,
        accept: Option<&str>,
        query: &str,
        variables: &Value,
    ) -> Result<(Option<Value>, Option<Vec<GraphQlError>>), HostingError> {
        let body = serde_json::json!({ "query": query, "variables": variables }).to_string();
        let answer = self.exchange("POST", url, url, token, accept, Some(&body))?;
        let response = checked(answer, token)?;
        let parsed: GraphQlResponse = serde_json::from_str(&response.body)
            .map_err(|_| HostingError::Io(PARSE_FAILED.to_string()))?;
        let data = parsed.data.filter(|data| !data.is_null());
        Ok((data, parsed.errors))
    }

    // One request and its whole answer, whatever the status. The token goes only when `url` has
    // the scheme, host and port of `origin`, the URL the caller named.
    fn exchange(
        &self,
        method: &str,
        origin: &str,
        url: &str,
        token: Option<&Secret>,
        accept: Option<&str>,
        body: Option<&str>,
    ) -> Result<Answer, HostingError> {
        if let Some(failure) = &self.tls_failure {
            if url::Url::parse(url).is_ok_and(|url| url.scheme() == "https") {
                return Err(HostingError::Io(format!(
                    "Cannot make HTTPS requests: the system TLS could not be set up ({failure})"
                )));
            }
        }
        let mut request = self.agent.request(method, url);
        if let Some(accept) = accept {
            request = request.set("Accept", accept);
        }
        if let Some(token) = token {
            if same_origin(origin, url) {
                // A `Secret` too, so that this copy is wiped as well.
                let value = Secret::new(format!("Bearer {}", token.expose()));
                request = request.set("Authorization", value.expose());
            }
        }
        let result = match body {
            Some(body) => request
                .set("Content-Type", "application/json")
                .send_string(body),
            None => request.call(),
        };
        let response = match result {
            Ok(response) => response,
            Err(ureq::Error::Status(_, response)) => response,
            Err(ureq::Error::Transport(transport)) => {
                return Err(hidden(transport_failure(&transport), token))
            }
        };
        let status = response.status();
        let reason = response.status_text().to_string();
        let headers = response
            .headers_names()
            .into_iter()
            .flat_map(|name| {
                response
                    .all(&name)
                    .into_iter()
                    .map(|value| (name.clone(), value.to_string()))
                    .collect::<Vec<_>>()
            })
            .collect();
        let body = response
            .into_string()
            .map_err(|error| hidden(io_failure(&error, false), token))?;
        Ok(Answer {
            status,
            reason,
            headers,
            body,
        })
    }
}

#[derive(serde::Deserialize)]
struct GraphQlResponse {
    data: Option<Value>,
    errors: Option<Vec<GraphQlError>>,
}

// GHGQLError: GitHub adds `type` to the standard error.
#[derive(serde::Deserialize)]
struct GraphQlError {
    message: String,
    #[serde(rename = "type")]
    kind: Option<String>,
}

// GithubErrorMessage: the JSON error of a REST answer, as GitHub sent it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct ErrorMessage {
    pub message: Option<String>,
    pub errors: Option<Vec<ErrorDetail>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct ErrorDetail {
    pub resource: Option<String>,
    pub field: Option<String>,
    pub code: Option<String>,
    pub message: Option<String>,
}

impl ErrorMessage {
    // A server that echoes the token back gets it written `***` here too.
    fn hidden(self, token: Option<&Secret>) -> Self {
        let hide = |text: Option<String>| text.map(|text| hide_token(text, token));
        Self {
            message: hide(self.message),
            errors: self.errors.map(|errors| {
                errors
                    .into_iter()
                    .map(|error| ErrorDetail {
                        resource: hide(error.resource),
                        field: hide(error.field),
                        code: hide(error.code),
                        message: hide(error.message),
                    })
                    .collect()
            }),
        }
    }

    // `getPresentableError`, with Java's `null` for what is missing. 241 joins the details with
    // `<br/>` for its HTML error panel; rama shows the text as text, so a line break.
    fn presentable(&self) -> String {
        let Some(errors) = &self.errors else {
            return self
                .message
                .clone()
                .unwrap_or_else(|| UNKNOWN_ERROR.to_string());
        };
        let text = |value: &Option<String>| value.clone().unwrap_or_else(|| "null".to_string());
        let mut presentable = self.message.clone().unwrap_or_default();
        for error in errors {
            presentable.push_str(&format!(
                "\n[{}; {}]{}: {}",
                text(&error.resource),
                text(&error.field),
                text(&error.code),
                text(&error.message)
            ));
        }
        presentable
    }
}

// checkResponseCode, then checkServerVersion.
fn checked(answer: Answer, token: Option<&Secret>) -> Result<Response, HostingError> {
    if answer.status >= 400 {
        return Err(status_failure(&answer, token));
    }
    if let Some(version) = header(&answer.headers, ENTERPRISE_VERSION) {
        enterprise_version_supported(version)?;
    }
    Ok(Response {
        status: answer.status,
        headers: answer.headers,
        body: answer.body,
    })
}

// GithubApiRequestExecutor.checkResponseCode. The JSON error is read only when the content type
// starts with `application/json`. An empty body counts as none.
fn status_failure(answer: &Answer, token: Option<&Secret>) -> HostingError {
    let hide = |text: String| hide_token(text, token);
    let status_line = format!("{} {}", answer.status, answer.reason);
    let error_text = Some(answer.body.as_str()).filter(|body| !body.is_empty());
    let json_error = error_text
        .filter(|_| {
            header(&answer.headers, "content-type")
                .is_some_and(|kind| kind.starts_with("application/json"))
        })
        .and_then(|body| serde_json::from_str::<ErrorMessage>(body).ok());
    match answer.status {
        401..=403 => {
            if let Some(error) = &json_error {
                if error
                    .message
                    .as_deref()
                    .is_some_and(|message| message.contains("API rate limit exceeded"))
                {
                    return HostingError::RateLimit(hide(error.presentable()));
                }
            }
            let message = json_error
                .map(|error| error.presentable())
                .or(error_text.map(str::to_string))
                .unwrap_or(status_line);
            HostingError::Authentication(hide(message))
        }
        _ => HostingError::Status {
            code: answer.status,
            reason: hide(answer.reason.clone()),
            // Kotlin writes a missing body as `null`.
            message: hide(
                json_error
                    .as_ref()
                    .map(ErrorMessage::presentable)
                    .or(error_text.map(str::to_string))
                    .unwrap_or_else(|| "null".to_string()),
            ),
            error: json_error.map(|error| error.hidden(token)),
        },
    }
}

// GQLQuery.throwException.
fn graphql_failure(errors: &[GraphQlError], token: Option<&Secret>) -> HostingError {
    let scopes = errors.iter().any(|error| {
        error
            .kind
            .as_deref()
            .is_some_and(|kind| kind.eq_ignore_ascii_case("INSUFFICIENT_SCOPES"))
    });
    if scopes {
        return HostingError::Scopes;
    }
    let message = match errors {
        [single] => single.message.clone(),
        _ => {
            let messages: Vec<&str> = errors.iter().map(|error| error.message.as_str()).collect();
            format!("[{}]", messages.join(", "))
        }
    };
    HostingError::GraphQl(hide_token(message, token))
}

// GHEServerVersionChecker.checkVersionSupported: 3.0 or later.
fn enterprise_version_supported(value: &str) -> Result<(), HostingError> {
    let mut parts = value.split('.');
    let number = |part: Option<&str>| part.and_then(|part| part.parse::<i32>().ok());
    let (Some(major), Some(minor)) = (number(parts.next()), number(parts.next())) else {
        return Err(HostingError::Io(
            "Could not determine GitHub Enterprise server version".to_string(),
        ));
    };
    if major > 3 || (major == 3 && minor >= 0) {
        return Ok(());
    }
    Err(HostingError::Io(format!(
        "Unsupported GitHub Enterprise server version {major}.{minor}. Earliest supported version is 3.0"
    )))
}

// LinkHttpHeaderValue.parse: sections split on `, `, each `<url>; rel="first|prev|next|last"`.
fn next_link(value: &str) -> Result<Option<String>, HostingError> {
    let mut next = None;
    for section in value.split(", ") {
        if section.trim().is_empty() {
            continue;
        }
        let parsed = section
            .strip_prefix('<')
            .and_then(|rest| rest.rsplit_once(">; rel=\""))
            .and_then(|(url, rel)| Some((url, rel.strip_suffix('"')?)))
            .filter(|(_, rel)| ["first", "prev", "next", "last"].contains(rel));
        match parsed {
            Some((url, "next")) => next = Some(url.to_string()),
            Some(_) => {}
            None => {
                return Err(HostingError::Io(format!(
                    "Incorrect header section format: {section}"
                )))
            }
        }
    }
    Ok(next)
}

// What a failed connection reads like: never the URL ureq puts first in its own text.
fn transport_failure(transport: &ureq::Transport) -> HostingError {
    use std::error::Error;

    let connecting = transport.kind() == ureq::ErrorKind::ConnectionFailed;
    if transport.kind() == ureq::ErrorKind::Dns {
        return HostingError::Unreachable;
    }
    let mut source = transport.source();
    while let Some(cause) = source {
        if let Some(error) = cause.downcast_ref::<std::io::Error>() {
            return io_failure(error, connecting);
        }
        source = cause.source();
    }
    let mut text = transport.kind().to_string();
    if let Some(message) = transport.message() {
        text.push_str(": ");
        text.push_str(message);
    }
    if let Some(cause) = transport.source() {
        text.push_str(&format!(": {cause}"));
    }
    HostingError::Io(text)
}

// Java's texts for the usual ones: `Connect timed out`, `Read timed out`, `Connection refused`.
fn io_failure(error: &std::io::Error, connecting: bool) -> HostingError {
    use std::io::ErrorKind;

    let text = match error.kind() {
        ErrorKind::TimedOut | ErrorKind::WouldBlock if connecting => {
            "Connect timed out".to_string()
        }
        ErrorKind::TimedOut | ErrorKind::WouldBlock => "Read timed out".to_string(),
        ErrorKind::ConnectionRefused => "Connection refused".to_string(),
        kind => {
            let text = kind.to_string();
            let mut characters = text.chars();
            match characters.next() {
                Some(first) => first.to_uppercase().chain(characters).collect(),
                None => text,
            }
        }
    };
    HostingError::Io(text)
}

fn hidden(error: HostingError, token: Option<&Secret>) -> HostingError {
    let hide = |text: String| hide_token(text, token);
    match error {
        HostingError::Authentication(text) => HostingError::Authentication(hide(text)),
        HostingError::RateLimit(text) => HostingError::RateLimit(hide(text)),
        HostingError::GraphQl(text) => HostingError::GraphQl(hide(text)),
        HostingError::Io(text) => HostingError::Io(hide(text)),
        HostingError::Status {
            code,
            reason,
            message,
            error,
        } => HostingError::Status {
            code,
            reason: hide(reason),
            message: hide(message),
            error: error.map(|error| error.hidden(token)),
        },
        other => other,
    }
}

// A server that echoes the token back gets it written `***` in the error.
fn hide_token(text: String, token: Option<&Secret>) -> String {
    match token.map(Secret::expose) {
        Some(token) if !token.is_empty() && text.contains(token) => text.replace(token, "***"),
        _ => text,
    }
}

// The first header of that name, ignoring case.
pub(crate) fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

// `new URL(new URL(base), location)`, with the URL crate that ureq connects with, so that where
// the next request goes and whether it gets the token come from one reading of the text.
fn resolve(base: &str, location: &str) -> Result<String, HostingError> {
    url::Url::parse(base)
        .and_then(|base| base.join(location))
        .map(String::from)
        .map_err(|_| HostingError::Io("Invalid redirect location".to_string()))
}

// Decision 12: the token is kept only on the same scheme, host and port, as the URL crate reads
// them (a `\\` ends the authority of an `http` or `https` URL, for example).
fn same_origin(first: &str, second: &str) -> bool {
    let (Ok(first), Ok(second)) = (url::Url::parse(first), url::Url::parse(second)) else {
        return false;
    };
    first.scheme() == second.scheme()
        && first.host().is_some()
        && first.host() == second.host()
        && first.port_or_known_default() == second.port_or_known_default()
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::time::Instant;

    use serde_json::json;

    use super::*;
    use crate::test_support::{FakeGitHub, Reply};

    const TOKEN: &str = "ghp_TOKEN0123456789";

    fn client() -> Client {
        Client::new(Duration::from_secs(5))
    }

    fn token() -> Secret {
        Secret::new(TOKEN.to_string())
    }

    fn json_error(status: u16, reason: &str, body: &str) -> HostingError {
        let fake = FakeGitHub::new();
        fake.reply(Reply::json("GET", "/api/v3/user", status, reason, body));
        client()
            .get(&format!("{}/api/v3/user", fake.url()), Some(&token()), None)
            .unwrap_err()
    }

    fn graphql_error(body: &str) -> HostingError {
        let fake = FakeGitHub::new();
        fake.reply(Reply::json("POST", "/api/graphql", 200, "OK", body));
        client()
            .graphql(
                &format!("{}/api/graphql", fake.url()),
                Some(&token()),
                None,
                "query { viewer { login } }",
                &json!({}),
            )
            .unwrap_err()
    }

    // Where nothing listens and no test can listen: port 1 is privileged and outside the
    // ephemeral range that `127.0.0.1:0` hands out, so the connection is refused every time.
    const REFUSING_PORT: u16 = 1;

    fn no_check() -> Result<(), HostingError> {
        Ok(())
    }

    #[test]
    fn errors_read_like_241() {
        let bad = json_error(401, "Unauthorized", r#"{"message":"Bad credentials"}"#);
        assert_eq!(
            bad,
            HostingError::Authentication("Bad credentials".to_string())
        );
        assert_eq!(bad.to_string(), "Request response: Bad credentials.");
        assert!(bad.is_authentication());

        let limit = json_error(
            403,
            "Forbidden",
            r#"{"message":"API rate limit exceeded for user ID 1."}"#,
        );
        assert_eq!(
            limit,
            HostingError::RateLimit("API rate limit exceeded for user ID 1.".to_string())
        );
        assert_eq!(limit.to_string(), "API rate limit exceeded for user ID 1.");

        let missing = json_error(404, "Not Found", r#"{"message":"Not Found"}"#);
        assert_eq!(
            missing,
            HostingError::Status {
                code: 404,
                reason: "Not Found".to_string(),
                message: "Not Found".to_string(),
                error: Some(ErrorMessage {
                    message: Some("Not Found".to_string()),
                    errors: None,
                }),
            }
        );
        assert!(!missing.is_authentication());
        assert_eq!(missing.to_string(), "404 Not Found - Not Found");

        // GithubErrorMessage.getPresentableError, with what is missing written `null`. 241 joins
        // the details with `<br/>` for its HTML panel; rama shows text, so a line break.
        let invalid = json_error(
            422,
            "Unprocessable Entity",
            r#"{"message":"Validation Failed","errors":[{"resource":"Issue","field":"title","code":"missing_field"}]}"#,
        );
        assert_eq!(
            invalid.to_string(),
            "422 Unprocessable Entity - Validation Failed\n[Issue; title]missing_field: null"
        );
        assert!(!invalid.to_string().contains('<'));
        // The parts stay, for the texts that read them (getLoadingErrorText).
        let HostingError::Status { error, .. } = &invalid else {
            panic!("{invalid:?}");
        };
        assert_eq!(
            error,
            &Some(ErrorMessage {
                message: Some("Validation Failed".to_string()),
                errors: Some(vec![ErrorDetail {
                    resource: Some("Issue".to_string()),
                    field: Some("title".to_string()),
                    code: Some("missing_field".to_string()),
                    message: None,
                }]),
            })
        );
        assert_eq!(
            json_error(403, "Forbidden", r#"{"documentation_url":"x"}"#).to_string(),
            "Request response: Unknown loading error."
        );

        // Not JSON: the raw body, or the status line when there is none.
        let fake = FakeGitHub::new();
        fake.script(vec![
            Reply::new("GET", "/a", 500, "Internal Server Error", &[], "boom"),
            Reply::new("GET", "/b", 401, "Unauthorized", &[], ""),
            Reply::new(
                "GET",
                "/c",
                401,
                "Unauthorized",
                &[("Content-Type", "text/plain")],
                r#"{"message":"Bad credentials"}"#,
            ),
        ]);
        let get = |path: &str| {
            client()
                .get(&format!("{}{path}", fake.url()), None, None)
                .unwrap_err()
                .to_string()
        };
        assert_eq!(get("/a"), "500 Internal Server Error - boom");
        assert_eq!(get("/b"), "Request response: 401 Unauthorized.");
        assert_eq!(
            get("/c"),
            r#"Request response: {"message":"Bad credentials"}."#
        );

        // GraphQL answers with 200 and its errors.
        let not_found = graphql_error(
            r#"{"data":null,"errors":[{"type":"NOT_FOUND","path":["repository"],"message":"Could not resolve to a Repository with the name 'o/r'."}]}"#,
        );
        assert_eq!(
            not_found,
            HostingError::GraphQl(
                "Could not resolve to a Repository with the name 'o/r'.".to_string()
            )
        );
        assert_eq!(
            not_found.to_string(),
            "Could not resolve to a Repository with the name 'o/r'."
        );
        assert_eq!(
            graphql_error(
                r#"{"errors":[{"message":"First problem"},{"type":"OTHER","message":"Second problem"}]}"#
            )
            .to_string(),
            "[First problem, Second problem]"
        );
        let scopes = graphql_error(
            r#"{"data":null,"errors":[{"type":"OTHER","message":"x"},{"type":"insufficient_scopes","message":"Your token has not been granted the required scopes"}]}"#,
        );
        assert_eq!(scopes, HostingError::Scopes);
        assert!(scopes.is_authentication());
        assert_eq!(
            scopes.to_string(),
            "Access token has not been granted the required scopes."
        );
        assert_eq!(
            graphql_error(r#"{"data":null}"#).to_string(),
            "Undefined request state - both result and errors are null"
        );

        // The 404-like case: data, but nothing at the path, and an error that says why.
        let fake = FakeGitHub::new();
        fake.script(vec![
            Reply::json(
                "POST",
                "/api/graphql",
                200,
                "OK",
                r#"{"data":{"repository":{"pullRequest":null}},"errors":[{"type":"NOT_FOUND","message":"Could not resolve to a PullRequest with the number of 7."}]}"#,
            ),
            Reply::json(
                "POST",
                "/api/graphql",
                200,
                "OK",
                r#"{"data":{"repository":{"pullRequest":null}}}"#,
            ),
            Reply::json(
                "POST",
                "/api/graphql",
                200,
                "OK",
                r#"{"data":{"repository":{"pullRequest":{"number":7}}},"errors":[{"message":"ignored"}]}"#,
            ),
        ]);
        let at = || {
            client().graphql_at(
                &format!("{}/api/graphql", fake.url()),
                None,
                None,
                "query",
                &json!({"number": 7}),
                &["repository", "pullRequest"],
            )
        };
        assert_eq!(
            at().unwrap_err().to_string(),
            "Could not resolve to a PullRequest with the number of 7."
        );
        assert_eq!(at().unwrap(), None);
        assert_eq!(at().unwrap(), Some(json!({"number": 7})));

        // A 200 that is not JSON, as a captive portal gives.
        let fake = FakeGitHub::new();
        fake.script(vec![
            Reply::new(
                "GET",
                "/api/v3/user",
                200,
                "OK",
                &[("Content-Type", "text/html")],
                "<html><body>Sign in to the Wi-Fi</body></html>",
            ),
            Reply::new(
                "POST",
                "/api/graphql",
                200,
                "OK",
                &[("Content-Type", "text/html")],
                "<html></html>",
            ),
        ]);
        let page = client()
            .get(&format!("{}/api/v3/user", fake.url()), None, None)
            .unwrap();
        assert_eq!(page.status, 200);
        assert_eq!(page.header("content-type"), Some("text/html"));
        assert_eq!(
            page.json().unwrap_err(),
            HostingError::Io("Can't parse GitHub response".to_string())
        );
        let graphql = client().graphql(
            &format!("{}/api/graphql", fake.url()),
            None,
            None,
            "query",
            &json!({}),
        );
        assert_eq!(
            graphql.unwrap_err().to_string(),
            "Can't parse GitHub response"
        );

        // GHEServerVersionChecker, on every response below 400 that has the header.
        let fake = FakeGitHub::new();
        let version = |value: &str| {
            Reply::new(
                "GET",
                "/api/v3/meta",
                200,
                "OK",
                &[
                    ("Content-Type", "application/json; charset=utf-8"),
                    ("x-github-enterprise-version", value),
                ],
                "{}",
            )
        };
        fake.script(vec![
            version("2.22"),
            version("3.0"),
            version("3.11.4"),
            version("abc"),
            version("3"),
        ]);
        let meta = || client().get(&format!("{}/api/v3/meta", fake.url()), None, None);
        assert_eq!(
            meta().unwrap_err().to_string(),
            "Unsupported GitHub Enterprise server version 2.22. Earliest supported version is 3.0"
        );
        assert!(meta().is_ok());
        assert!(meta().is_ok());
        assert_eq!(
            meta().unwrap_err().to_string(),
            "Could not determine GitHub Enterprise server version"
        );
        assert_eq!(
            meta().unwrap_err().to_string(),
            "Could not determine GitHub Enterprise server version"
        );

        // Nothing listens, and the host is unknown (GithubBundle 331).
        let port = REFUSING_PORT;
        let refused = client()
            .get(&format!("http://127.0.0.1:{port}/user"), None, None)
            .unwrap_err();
        assert_eq!(refused, HostingError::Io("Connection refused".to_string()));
        assert_eq!(
            HostingError::Unreachable.to_string(),
            "Server is unreachable"
        );
    }

    #[test]
    fn requests_carry_rama_s_headers() {
        let fake = FakeGitHub::new();
        fake.script(vec![
            Reply::json("GET", "/api/v3/user", 200, "OK", r#"{"login":"octocat"}"#),
            Reply::json("GET", "/api/v3/meta", 200, "OK", "{}"),
            Reply::json(
                "POST",
                "/api/graphql",
                200,
                "OK",
                r#"{"data":{"viewer":null}}"#,
            ),
        ]);
        let user = client()
            .get(
                &format!("{}/api/v3/user", fake.url()),
                Some(&token()),
                Some("application/vnd.github.v3+json"),
            )
            .unwrap();
        assert_eq!(user.json().unwrap(), json!({"login": "octocat"}));
        client()
            .get(&format!("{}/api/v3/meta", fake.url()), None, None)
            .unwrap();
        let data = client()
            .graphql(
                &format!("{}/api/graphql", fake.url()),
                Some(&token()),
                Some("application/vnd.github.shadow-cat-preview+json"),
                "query { viewer { login } }",
                &json!({"a": 1}),
            )
            .unwrap();
        assert_eq!(data, json!({"viewer": null}));

        let requests = fake.requests();
        assert_eq!(requests.len(), 3);
        for request in &requests {
            assert_eq!(request.header("user-agent"), Some("rama"));
            assert_eq!(request.header("accept-encoding"), None);
        }
        assert_eq!(
            requests[0].header("authorization"),
            Some(format!("Bearer {TOKEN}").as_str())
        );
        assert_eq!(
            requests[0].header("accept"),
            Some("application/vnd.github.v3+json")
        );
        assert_eq!(requests[1].header("authorization"), None);
        assert_ne!(
            requests[1].header("accept"),
            Some("application/vnd.github.v3+json")
        );
        assert_eq!(requests[2].header("content-type"), Some("application/json"));
        assert_eq!(
            requests[2].header("accept"),
            Some("application/vnd.github.shadow-cat-preview+json")
        );
        let body: Value = serde_json::from_str(&requests[2].body).unwrap();
        assert_eq!(
            body,
            json!({"query": "query { viewer { login } }", "variables": {"a": 1}})
        );
    }

    #[test]
    fn a_redirect_to_another_host_drops_the_token() {
        let first = FakeGitHub::new();
        let second = FakeGitHub::new();
        first.script(vec![
            Reply::new("GET", "/start", 302, "Found", &[("Location", "/moved")], ""),
            Reply::new(
                "GET",
                "/moved",
                301,
                "Moved Permanently",
                &[("Location", &format!("{}/landing?x=1", second.url()))],
                "",
            ),
            Reply::json("GET", "/back", 200, "OK", "{}"),
        ]);
        second.script(vec![Reply::new(
            "GET",
            "/landing?x=1",
            307,
            "Temporary Redirect",
            &[("Location", &format!("{}/back", first.url()))],
            "",
        )]);

        let response = client()
            .get(&format!("{}/start", first.url()), Some(&token()), None)
            .unwrap();
        assert_eq!(response.status, 200);

        let bearer = format!("Bearer {TOKEN}");
        let on_first = first.requests();
        let targets: Vec<&str> = on_first.iter().map(|r| r.target.as_str()).collect();
        assert_eq!(targets, ["/start", "/moved", "/back"]);
        for request in &on_first {
            assert_eq!(request.header("authorization"), Some(bearer.as_str()));
        }
        // Another port of the same host is another server.
        let on_second = second.requests();
        assert_eq!(on_second.len(), 1);
        assert_eq!(on_second[0].header("authorization"), None);
        assert!(on_second[0]
            .headers
            .iter()
            .all(|(_, value)| !value.contains(TOKEN)));
    }

    // A `\\` ends the authority for `http`/`https` in the URL crate, which ureq connects with:
    // `http://<second>\\@<first>/x` goes to the second server, whatever reads it otherwise.
    #[test]
    fn a_redirect_that_hides_another_host_behind_a_backslash_drops_the_token() {
        let first = FakeGitHub::new();
        let second = FakeGitHub::new();
        let first_address = first.url().trim_start_matches("http://").to_string();
        let second_address = second.url().trim_start_matches("http://").to_string();
        let absolute = format!("http://{second_address}\\@{first_address}/x");
        let relative = format!("//{second_address}\\@{first_address}/y");
        let path = |location: &str| {
            let base = url::Url::parse(&first.url()).unwrap();
            base.join(location).unwrap().path().to_string()
        };
        first.script(vec![
            Reply::new("GET", "/a", 302, "Found", &[("Location", &absolute)], ""),
            Reply::new("GET", "/b", 302, "Found", &[("Location", &relative)], ""),
        ]);
        second.script(vec![
            Reply::json("GET", &path(&absolute), 200, "OK", "{}"),
            Reply::json("GET", &path(&relative), 200, "OK", "{}"),
        ]);
        for start in ["/a", "/b"] {
            client()
                .get(&format!("{}{start}", first.url()), Some(&token()), None)
                .unwrap();
        }
        let bearer = format!("Bearer {TOKEN}");
        for request in first.requests() {
            assert_eq!(request.header("authorization"), Some(bearer.as_str()));
        }
        let on_second = second.requests();
        assert_eq!(on_second.len(), 2);
        for request in &on_second {
            assert_eq!(request.header("authorization"), None, "{}", request.target);
        }
    }

    #[test]
    fn a_redirect_loop_stops_at_ten() {
        let fake = FakeGitHub::new();
        for _ in 0..10 {
            fake.reply(Reply::new(
                "GET",
                "/loop",
                302,
                "Found",
                &[("Location", "/loop")],
                "",
            ));
        }
        let error = client()
            .get(&format!("{}/loop", fake.url()), Some(&token()), None)
            .unwrap_err();
        assert_eq!(
            error,
            HostingError::Io("Connection failed: too many redirects".to_string())
        );
        assert_eq!(fake.requests().len(), 10);
    }

    #[test]
    fn the_token_never_shows_in_an_error_or_debug() {
        let fake = FakeGitHub::new();
        let echo = format!(r#"{{"message":"Bad credentials {TOKEN}"}}"#);
        let limit = format!(r#"{{"message":"API rate limit exceeded for {TOKEN}"}}"#);
        let raw = format!("no such token {TOKEN}");
        let details = format!(
            r#"{{"message":"Invalid {TOKEN}","errors":[{{"resource":"{TOKEN}","field":"f","code":"c","message":"{TOKEN}"}}]}}"#
        );
        let graphql =
            format!(r#"{{"errors":[{{"message":"bad {TOKEN}"}},{{"message":"{TOKEN}"}}]}}"#);
        let link = format!("<{TOKEN}>; rel=\"broken\"");
        fake.script(vec![
            Reply::json("GET", "/a", 401, "Unauthorized", &echo),
            Reply::json("GET", "/b", 403, "Forbidden", &limit),
            Reply::new("GET", "/c", 404, "Not Found", &[], &raw),
            Reply::json("GET", "/d", 422, "Unprocessable Entity", &details),
            Reply::json("POST", "/e", 200, "OK", &graphql),
            Reply::new(
                "GET",
                "/f",
                200,
                "OK",
                &[
                    ("Content-Type", "application/json"),
                    ("x-github-enterprise-version", TOKEN),
                ],
                "[]",
            ),
            Reply::new(
                "GET",
                "/g",
                200,
                "OK",
                &[("Content-Type", "application/json"), ("Link", &link)],
                "[]",
            ),
        ]);
        let secret = token();
        let port = REFUSING_PORT;
        let base = fake.url();
        let get = |path: &str| client().get(&format!("{base}{path}"), Some(&secret), None);
        let mut errors = vec![
            get("/a").unwrap_err(),
            get("/b").unwrap_err(),
            get("/c").unwrap_err(),
            get("/d").unwrap_err(),
            client()
                .graphql(&format!("{base}/e"), Some(&secret), None, "q", &json!({}))
                .unwrap_err(),
            get("/f").unwrap_err(),
            client()
                .pages_rest(&format!("{base}/g"), Some(&secret), None, &no_check)
                .unwrap_err(),
            // The token in the query of a URL nobody answers.
            client()
                .get(
                    &format!("http://127.0.0.1:{port}/user?access_token={TOKEN}"),
                    Some(&secret),
                    None,
                )
                .unwrap_err(),
        ];
        errors.push(HostingError::Scopes);
        errors.push(HostingError::Unreachable);
        for error in &errors {
            for text in [
                format!("{error}"),
                format!("{error:?}"),
                format!("{error:#?}"),
            ] {
                assert!(!text.contains(TOKEN), "{text}");
                assert!(!text.contains("Bearer"), "{text}");
            }
        }
        assert_eq!(
            errors[0].to_string(),
            "Request response: Bad credentials ***."
        );
        for text in [format!("{secret:?}"), format!("{:?}", Some(&secret))] {
            assert!(!text.contains(TOKEN), "{text}");
        }

        // The fake saw the token only in the header that carries it.
        for request in fake.requests() {
            assert!(!request.target.contains(TOKEN));
            assert!(!request.body.contains(TOKEN));
            for (name, value) in &request.headers {
                if !name.eq_ignore_ascii_case("authorization") {
                    assert!(!value.contains(TOKEN), "{name}");
                }
            }
        }
    }

    #[test]
    fn rest_pages_follow_link_next() {
        let fake = FakeGitHub::new();
        let base = fake.url();
        let first_link = format!(
            "<{base}/api/v3/repos/o/r/labels?per_page=100&page=2>; rel=\"next\", <{base}/api/v3/repos/o/r/labels?per_page=100&page=3>; rel=\"last\""
        );
        let second_link = format!(
            "<{base}/api/v3/repos/o/r/labels?per_page=100&page=1>; rel=\"prev\", <{base}/api/v3/repos/o/r/labels?per_page=100&page=3>; rel=\"next\""
        );
        let third_link = format!(
            "<{base}/api/v3/repos/o/r/labels?per_page=100&page=1>; rel=\"first\", <{base}/api/v3/repos/o/r/labels?per_page=100&page=2>; rel=\"prev\""
        );
        let page = |target: &str, link: &str, body: &str| {
            Reply::new(
                "GET",
                target,
                200,
                "OK",
                &[
                    ("Content-Type", "application/json; charset=utf-8"),
                    ("Link", link),
                ],
                body,
            )
        };
        fake.script(vec![
            page(
                "/api/v3/repos/o/r/labels?per_page=100",
                &first_link,
                r#"[{"name":"a"},{"name":"b"}]"#,
            ),
            page(
                "/api/v3/repos/o/r/labels?per_page=100&page=2",
                &second_link,
                r#"[{"name":"c"}]"#,
            ),
            page(
                "/api/v3/repos/o/r/labels?per_page=100&page=3",
                &third_link,
                r#"[]"#,
            ),
            Reply::json(
                "GET",
                "/api/v3/repos/o/r/labels?per_page=100&page=4",
                200,
                "OK",
                "[1]",
            ),
            Reply::new(
                "GET",
                "/api/v3/repos/o/r/pulls?page=1&per_page=1",
                200,
                "OK",
                &[("Content-Type", "application/json"), ("ETag", "W/\"abc\"")],
                "[]",
            ),
            Reply::json(
                "GET",
                "/api/v3/repos/o/r/pulls?page=1&per_page=1",
                200,
                "OK",
                "[]",
            ),
        ]);
        let all = client()
            .pages_rest(
                &format!("{base}/api/v3/repos/o/r/labels?per_page=100"),
                Some(&token()),
                Some("application/vnd.github.v3+json"),
                &no_check,
            )
            .unwrap();
        assert_eq!(
            all,
            [
                json!({"name":"a"}),
                json!({"name":"b"}),
                json!({"name":"c"})
            ]
        );
        // Without a `Link`, one page.
        let single = client()
            .pages_rest(
                &format!("{base}/api/v3/repos/o/r/labels?per_page=100&page=4"),
                None,
                None,
                &no_check,
            )
            .unwrap();
        assert_eq!(single, [json!(1)]);

        let pulls = format!("{base}/api/v3/repos/o/r/pulls?page=1&per_page=1");
        assert_eq!(
            client().etag(&pulls, Some(&token())).unwrap().as_deref(),
            Some("W/\"abc\"")
        );
        assert_eq!(client().etag(&pulls, Some(&token())).unwrap(), None);

        let requests = fake.requests();
        for request in &requests[..3] {
            assert_eq!(
                request.header("accept"),
                Some("application/vnd.github.v3+json")
            );
            assert_eq!(
                request.header("authorization"),
                Some(format!("Bearer {TOKEN}").as_str())
            );
        }
        // The ETag request sends no `Accept` of GitHub's.
        assert_ne!(
            requests[4].header("accept"),
            Some("application/vnd.github.v3+json")
        );
    }

    #[test]
    fn graphql_pages_follow_end_cursor() {
        let fake = FakeGitHub::new();
        fake.script(vec![
            Reply::json(
                "POST",
                "/api/graphql",
                200,
                "OK",
                r#"{"data":{"repository":{"rules":{"nodes":[{"pattern":"main"},{"pattern":"release/*"}],"pageInfo":{"startCursor":"A","hasPreviousPage":false,"endCursor":"B","hasNextPage":true}}}}}"#,
            ),
            Reply::json(
                "POST",
                "/api/graphql",
                200,
                "OK",
                r#"{"data":{"repository":{"rules":{"nodes":[{"pattern":"prod"}],"pageInfo":{"startCursor":"C","hasPreviousPage":true,"endCursor":"D","hasNextPage":false}}}}}"#,
            ),
        ]);
        let query =
            "query($owner: String!, $name: String!, $pageSize: Int = 100, $cursor: String) { … }";
        let nodes = client()
            .graphql_pages(
                &format!("{}/api/graphql", fake.url()),
                Some(&token()),
                None,
                query,
                &json!({"owner": "o", "name": "r", "pageSize": 100}),
                &["repository", "rules"],
                &no_check,
            )
            .unwrap();
        assert_eq!(
            nodes,
            [
                json!({"pattern": "main"}),
                json!({"pattern": "release/*"}),
                json!({"pattern": "prod"})
            ]
        );

        let bodies: Vec<Value> = fake
            .requests()
            .iter()
            .map(|request| serde_json::from_str(&request.body).unwrap())
            .collect();
        assert_eq!(
            bodies,
            [
                json!({"query": query, "variables": {"owner": "o", "name": "r", "pageSize": 100, "cursor": null}}),
                json!({"query": query, "variables": {"owner": "o", "name": "r", "pageSize": 100, "cursor": "B"}}),
            ]
        );
    }

    // The caller's check runs before each page: cancelled after the first, the second is never
    // asked for.
    #[test]
    fn paging_stops_when_the_task_is_cancelled() {
        let fake = FakeGitHub::new();
        let base = fake.url();
        let link = format!("<{base}/items?page=2>; rel=\"next\"");
        fake.script(vec![
            Reply::new(
                "GET",
                "/items",
                200,
                "OK",
                &[("Content-Type", "application/json"), ("Link", &link)],
                "[1]",
            ),
            Reply::json(
                "POST",
                "/api/graphql",
                200,
                "OK",
                r#"{"data":{"search":{"nodes":[1],"pageInfo":{"endCursor":"A","hasNextPage":true}}}}"#,
            ),
        ]);
        let pages = std::cell::Cell::new(0);
        let cancel_after_one = || {
            pages.set(pages.get() + 1);
            if pages.get() > 1 {
                Err(HostingError::Cancelled)
            } else {
                Ok(())
            }
        };
        let rest = client().pages_rest(&format!("{base}/items"), None, None, &cancel_after_one);
        assert_eq!(rest, Err(HostingError::Cancelled));
        assert_eq!(rest.unwrap_err().to_string(), "The operation was cancelled");

        pages.set(0);
        let graphql = client().graphql_pages(
            &format!("{base}/api/graphql"),
            None,
            None,
            "query",
            &json!({}),
            &["search"],
            &cancel_after_one,
        );
        assert_eq!(graphql, Err(HostingError::Cancelled));
        assert_eq!(fake.requests().len(), 2);
        fake.finish();
    }

    // Without the system's TLS, an `https://` request fails with a text that says so, before any
    // connection; `http://` still works.
    #[test]
    fn a_client_without_tls_says_so_on_https() {
        let limit = Duration::from_secs(5);
        let broken = Client::with_connector(limit, limit, Err("no trust store".to_string()));
        let port = REFUSING_PORT;
        assert_eq!(
            broken
                .get(
                    &format!("https://127.0.0.1:{port}/user"),
                    Some(&token()),
                    None
                )
                .unwrap_err(),
            HostingError::Io(
                "Cannot make HTTPS requests: the system TLS could not be set up (no trust store)"
                    .to_string()
            )
        );
        let fake = FakeGitHub::new();
        fake.reply(Reply::json("GET", "/user", 200, "OK", "{}"));
        assert!(broken
            .get(&format!("{}/user", fake.url()), None, None)
            .is_ok());
    }

    #[test]
    #[should_panic(expected = "was never asked for [\"GET /second\"]")]
    fn a_reply_nobody_asked_for_fails_the_test() {
        let fake = FakeGitHub::new();
        fake.script(vec![
            Reply::json("GET", "/first", 200, "OK", "{}"),
            Reply::json("GET", "/second", 200, "OK", "{}"),
        ]);
        client()
            .get(&format!("{}/first", fake.url()), None, None)
            .unwrap();
        fake.finish();
    }

    // `Connection timeout: 0` is `setConnectTimeout(0)` in 241: no limit, not an instant failure.
    #[test]
    fn a_zero_connect_timeout_means_none() {
        let fake = FakeGitHub::new();
        fake.reply(Reply::json(
            "GET",
            "/api/v3/user",
            200,
            "OK",
            r#"{"login":"octocat"}"#,
        ));
        let user = Client::new(Duration::ZERO)
            .get(&format!("{}/api/v3/user", fake.url()), Some(&token()), None)
            .unwrap();
        assert_eq!(user.json().unwrap(), json!({"login": "octocat"}));
        fake.finish();
    }

    // The read limit, provoked by a server that accepts and never answers: the whole request
    // ends within the connect and read limits. A connect timeout cannot be provoked offline on
    // this machine: macOS resets a connection to a full loopback queue at once instead of
    // dropping it, and an unroutable address would go to the network.
    #[test]
    fn a_slow_server_times_out_within_the_connect_and_read_limits() {
        let limit = Duration::from_millis(300);
        let slow = Client::with_timeouts(limit, limit);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let silent = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_secs(1));
            drop(stream);
        });
        let started = Instant::now();
        let error = slow
            .get(&format!("http://{address}/user"), Some(&token()), None)
            .unwrap_err();
        let waited = started.elapsed();
        assert_eq!(error, HostingError::Io("Read timed out".to_string()));
        assert!(waited >= limit, "{waited:?}");
        assert!(waited < limit * 2 + Duration::from_secs(1), "{waited:?}");
        silent.join().unwrap();
    }
}
