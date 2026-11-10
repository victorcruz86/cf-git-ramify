// A GitHub server: github.com or a GitHub Enterprise, with an optional port and path prefix.
// Port of GithubServerPath (plugins/github, api/GithubServerPath.java at idea/241.14494.240).

use std::fmt;
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

pub const GITHUB_COM: &str = "github.com";

// `use_http`: `Some(true)` for `http://`, `Some(false)` for `https://`, `None` when the text had
// no scheme (which means https). The host is kept in lower case.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubServer {
    pub use_http: Option<bool>,
    pub host: String,
    pub port: Option<u16>,
    pub suffix: Option<String>,
}

// The texts of GithubLoginPanel's checks (GithubBundle 34 and 35).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerError {
    Empty,
    Invalid,
}

impl fmt::Display for ServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            ServerError::Empty => "Server cannot be empty",
            ServerError::Invalid => "Server URL is not valid",
        })
    }
}

impl GithubServer {
    pub fn github_com() -> Self {
        Self {
            use_http: None,
            host: GITHUB_COM.to_string(),
            port: None,
            suffix: None,
        }
    }

    // GithubLoginPanel trims the text, and GithubServerPath.from parses it. The port must fit in
    // 16 bits: 241 takes any `int` and fails later, when it builds the URI.
    pub fn parse(text: &str) -> Result<Self, ServerError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(ServerError::Empty);
        }
        let groups = url_regex().captures(text).ok_or(ServerError::Invalid)?;
        let use_http = groups
            .get(1)
            .map(|scheme| scheme.as_str().eq_ignore_ascii_case("http://"));
        let host = groups[2].to_lowercase();
        let port = match groups.get(4) {
            Some(port) => Some(
                port.as_str()
                    .parse::<u16>()
                    .map_err(|_| ServerError::Invalid)?,
            ),
            None => None,
        };
        let suffix = groups
            .get(5)
            .map(|path| path.as_str().to_string())
            .filter(|path| !path.is_empty());
        Ok(Self {
            use_http,
            host,
            port,
            suffix,
        })
    }

    pub fn is_github_com(&self) -> bool {
        self.host.eq_ignore_ascii_case(GITHUB_COM)
    }

    // `toApiUrl`: github.com's API is on `api.github.com`, an Enterprise's under `/api/v3`.
    pub fn api_url(&self) -> String {
        if self.is_github_com() {
            format!("{}://api.{}", self.scheme(), self.address())
        } else {
            format!("{}://{}/api/v3", self.scheme(), self.address())
        }
    }

    // `toGraphQLUrl`.
    pub fn graphql_url(&self) -> String {
        if self.is_github_com() {
            format!("{}://api.{}/graphql", self.scheme(), self.address())
        } else {
            format!("{}://{}/api/graphql", self.scheme(), self.address())
        }
    }

    // `toUrl`: where the web pages are.
    pub fn web_url(&self) -> String {
        format!("{}://{}", self.scheme(), self.address())
    }

    // Whether the URL crate, which ureq connects with, reads in `api_url()` the host and port
    // parsed here. The host of the regex lets `\` and `@` through, so `ghe.example\@evil.test`
    // would show one host while the requests, and the token, went to `ghe.example`.
    pub fn url_crate_reads_the_same_host(&self) -> bool {
        let Ok(url) = url::Url::parse(&self.api_url()) else {
            return false;
        };
        let host = if self.is_github_com() {
            format!("api.{}", self.host)
        } else {
            self.host.clone()
        };
        let port = self
            .port
            .unwrap_or(if self.scheme() == "http" { 80 } else { 443 });
        url.host_str() == Some(host.as_str())
            && url.port_or_known_default() == Some(port)
            && url.username().is_empty()
            && url.password().is_none()
    }

    // `toString`: the scheme only when the text had one, so `github.com` reads `github.com`.
    pub fn display(&self) -> String {
        match self.use_http {
            Some(_) => self.web_url(),
            None => self.address(),
        }
    }

    // `equals(other, ignoreProtocol = true)`.
    pub fn same_ignoring_scheme(&self, other: &GithubServer) -> bool {
        self.host == other.host && self.port == other.port && self.suffix == other.suffix
    }

    pub fn scheme(&self) -> &'static str {
        if self.use_http == Some(true) {
            "http"
        } else {
            "https"
        }
    }

    // Host, port and prefix, with no scheme: the server part of an account's id.
    pub fn address(&self) -> String {
        let port = self.port.map(|port| format!(":{port}")).unwrap_or_default();
        let suffix = self.suffix.as_deref().unwrap_or_default();
        format!("{}{port}{suffix}", self.host)
    }
}

// GithubServerPath.URL_REGEX, which `matches` the whole text: 1 scheme, 2 host, 4 port, 5 path.
// `[0-9]` because Java's `\d` is ASCII only.
fn url_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?i)\A(https?://)?([^/?:]+)(:([0-9]+))?((/[^/?#]+)*)?/?\z").unwrap()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(text: &str) -> GithubServer {
        GithubServer::parse(text).unwrap()
    }

    // (typed text, use_http, host, port, suffix, API, GraphQL, web, display)
    #[test]
    fn server_paths_follow_241() {
        #[allow(clippy::type_complexity)]
        let cases: [(
            &str,
            Option<bool>,
            &str,
            Option<u16>,
            Option<&str>,
            &str,
            &str,
            &str,
            &str,
        ); 6] = [
            (
                "github.com",
                None,
                "github.com",
                None,
                None,
                "https://api.github.com",
                "https://api.github.com/graphql",
                "https://github.com",
                "github.com",
            ),
            (
                "https://GitHub.com/",
                Some(false),
                "github.com",
                None,
                None,
                "https://api.github.com",
                "https://api.github.com/graphql",
                "https://github.com",
                "https://github.com",
            ),
            (
                "http://ghe.example.com",
                Some(true),
                "ghe.example.com",
                None,
                None,
                "http://ghe.example.com/api/v3",
                "http://ghe.example.com/api/graphql",
                "http://ghe.example.com",
                "http://ghe.example.com",
            ),
            (
                "ghe.example.com:8443",
                None,
                "ghe.example.com",
                Some(8443),
                None,
                "https://ghe.example.com:8443/api/v3",
                "https://ghe.example.com:8443/api/graphql",
                "https://ghe.example.com:8443",
                "ghe.example.com:8443",
            ),
            (
                "GHE.Example.com/prefix/",
                None,
                "ghe.example.com",
                None,
                Some("/prefix"),
                "https://ghe.example.com/prefix/api/v3",
                "https://ghe.example.com/prefix/api/graphql",
                "https://ghe.example.com/prefix",
                "ghe.example.com/prefix",
            ),
            (
                "  HTTPS://ghe.example.com:8443/a/b  ",
                Some(false),
                "ghe.example.com",
                Some(8443),
                Some("/a/b"),
                "https://ghe.example.com:8443/a/b/api/v3",
                "https://ghe.example.com:8443/a/b/api/graphql",
                "https://ghe.example.com:8443/a/b",
                "https://ghe.example.com:8443/a/b",
            ),
        ];
        for (text, use_http, host, port, suffix, api, graphql, web, display) in cases {
            let parsed = server(text);
            assert_eq!(
                parsed,
                GithubServer {
                    use_http,
                    host: host.to_string(),
                    port,
                    suffix: suffix.map(str::to_string),
                },
                "{text}"
            );
            assert_eq!(parsed.api_url(), api, "{text}");
            assert_eq!(parsed.graphql_url(), graphql, "{text}");
            assert_eq!(parsed.web_url(), web, "{text}");
            assert_eq!(parsed.display(), display, "{text}");
        }

        // github.com through a port or a prefix is still github.com, and its API keeps them.
        let odd = server("github.com:8080/x");
        assert!(odd.is_github_com());
        assert_eq!(odd.api_url(), "https://api.github.com:8080/x");
        assert_eq!(odd.graphql_url(), "https://api.github.com:8080/x/graphql");

        assert_eq!(GithubServer::github_com(), server("github.com"));
        assert!(GithubServer::github_com().is_github_com());
        assert!(!server("ghe.example.com").is_github_com());

        // GithubLoginPanel's checks, first failure only (GithubBundle 34 and 35).
        assert_eq!(GithubServer::parse(""), Err(ServerError::Empty));
        assert_eq!(GithubServer::parse(" \t "), Err(ServerError::Empty));
        assert_eq!(ServerError::Empty.to_string(), "Server cannot be empty");
        for invalid in [
            "https://",
            "ftp://github.com",
            "github.com:port",
            "github.com?x=1",
            "github.com/a//b",
            "github.com:99999",
        ] {
            assert_eq!(
                GithubServer::parse(invalid),
                Err(ServerError::Invalid),
                "{invalid}"
            );
        }
        assert_eq!(ServerError::Invalid.to_string(), "Server URL is not valid");
    }

    #[test]
    fn a_host_the_url_crate_reads_otherwise_is_told_apart() {
        for same in [
            "github.com",
            "https://GitHub.com:443/",
            "github.com:8080/x",
            "http://ghe.example.com",
            "ghe.example.com:8443/a/b",
            "http://127.0.0.1:8080",
        ] {
            assert!(server(same).url_crate_reads_the_same_host(), "{same}");
        }
        for other in [
            r"ghe.example\@evil.test",
            r"http://ghe.example\@evil.test",
            "ghe.example.com@127.0.0.1",
            "octo@ghe.example.com:8443",
            r"ghe.example.com\evil.test",
            "ghe.example.com#evil.test",
        ] {
            assert!(
                GithubServer::parse(other)
                    .is_ok_and(|parsed| !parsed.url_crate_reads_the_same_host()),
                "{other}"
            );
        }
    }

    #[test]
    fn servers_are_the_same_ignoring_only_the_scheme() {
        let plain = server("ghe.example.com");
        assert!(plain.same_ignoring_scheme(&server("http://GHE.example.com")));
        assert!(plain.same_ignoring_scheme(&server("https://ghe.example.com/")));
        assert!(!plain.same_ignoring_scheme(&server("ghe.example.com:443")));
        assert!(!plain.same_ignoring_scheme(&server("ghe.example.com/prefix")));
        assert!(!plain.same_ignoring_scheme(&server("other.example.com")));
        // Full equality also compares the scheme, as GithubServerPath.equals does.
        assert_ne!(plain, server("http://ghe.example.com"));
    }
}
