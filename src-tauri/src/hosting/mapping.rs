// Which GitHub repository each remote points to. Ports of GitHostingUrlUtil (git4idea,
// remote/hosting/GitHostingUrlUtil.kt: getUriFromRemoteUrl, match), GithubUrlUtil
// (getUserAndRepositoryFromRemoteUrl), GHRepositoryCoordinates, GHRepositoryPath and
// GHGitRepositoryMapping, at idea/241.14494.240.

use serde::Serialize;

use super::server::GithubServer;
use crate::remotes::Remote;

// Equal when the servers are the same ignoring the scheme and the owner and name are the same
// ignoring case, as GHRepositoryCoordinates and GHRepositoryPath compare them. The case is kept
// as the remote URL writes it.
#[derive(Debug, Clone, Serialize)]
pub struct RepoCoordinates {
    pub server: GithubServer,
    pub owner: String,
    pub name: String,
}

impl PartialEq for RepoCoordinates {
    fn eq(&self, other: &Self) -> bool {
        self.server.same_ignoring_scheme(&other.server)
            && same_ignoring_case(&self.owner, &other.owner)
            && same_ignoring_case(&self.name, &other.name)
    }
}

impl Eq for RepoCoordinates {}

// A remote that points to a repository of a known server: the remote's name and the URL that
// matched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GithubMapping {
    pub coordinates: RepoCoordinates,
    pub remote: String,
    pub url: String,
}

// GHHostedRepositoriesManager: each remote's fetch URL goes to the first server that matches it,
// and the URL gives the owner and the name. Remotes on one repository give one mapping, the
// first one's. A remote with no server, or whose URL has no owner, gives none.
pub fn mappings(remotes: &[Remote], servers: &[GithubServer]) -> Vec<GithubMapping> {
    let mut found: Vec<GithubMapping> = Vec::new();
    for remote in remotes {
        let Some(server) = servers
            .iter()
            .find(|server| matches_server(server, &remote.url))
        else {
            continue;
        };
        let Some((owner, name)) = owner_and_name(&remote.url) else {
            continue;
        };
        let coordinates = RepoCoordinates {
            server: server.clone(),
            owner,
            name,
        };
        if found
            .iter()
            .any(|mapping| mapping.coordinates == coordinates)
        {
            continue;
        }
        found.push(GithubMapping {
            coordinates,
            remote: remote.name.clone(),
            url: remote.url.clone(),
        });
    }
    found
}

// GHRepositoryCoordinates.getWebURI: the server's web URL and `owner/name`.
pub fn web_base(mapping: &GithubMapping) -> String {
    let coordinates = &mapping.coordinates;
    format!(
        "{}/{}/{}",
        coordinates.server.web_url(),
        coordinates.owner,
        coordinates.name
    )
}

// Kotlin's `equals(other, ignoreCase = true)`.
fn same_ignoring_case(left: &str, right: &str) -> bool {
    left.to_lowercase() == right.to_lowercase()
}

// GitHostingUrlUtil.match with `server.toURI()`: the hosts are the same ignoring case and, when
// the server has a path prefix, the remote's path starts with it ignoring case. Neither the
// scheme nor the port is compared, and the prefix has no segment boundary.
pub(crate) fn matches_server(server: &GithubServer, remote_url: &str) -> bool {
    let Some(remote) = remote_uri(remote_url) else {
        return false;
    };
    if !same_ignoring_case(&server.host, &remote.host) {
        return false;
    }
    match server.suffix.as_deref() {
        None | Some("/") => true,
        Some(prefix) => remote
            .path
            .to_lowercase()
            .starts_with(&prefix.to_lowercase()),
    }
}

// GitHostingUrlUtil.removeProtocolPrefix: after the first `@`, else after `://`.
fn remove_protocol_prefix(url: &str) -> &str {
    if let Some(at) = url.find('@') {
        return &url[at + 1..];
    }
    match url.find("://") {
        Some(index) => &url[index + 3..],
        None => url,
    }
}

// GithubUrlUtil.getUserAndRepositoryFromRemoteUrl: one trailing `/` and `.git` go; the name is
// after the last `/`, and the owner between the `/` or `:` before it and that `/`.
fn owner_and_name(remote_url: &str) -> Option<(String, String)> {
    let url = remote_url.strip_suffix('/').unwrap_or(remote_url);
    let url = url.strip_suffix(".git").unwrap_or(url);
    let url = remove_protocol_prefix(url);
    let last = url.rfind('/')?;
    let before = &url[..last];
    let start = before.rfind(['/', ':'])?;
    let owner = &url[start + 1..last];
    let name = &url[last + 1..];
    if owner.is_empty() || name.is_empty() {
        return None;
    }
    Some((owner.to_string(), name.to_string()))
}

// What `java.net.URI` gives for a remote: the host of a server-based authority and the decoded
// path.
struct RemoteUri {
    host: String,
    path: String,
}

// GitHostingUrlUtil.getUriFromRemoteUrl. Every trailing `/` goes, then one `.git`. An scp-like
// URL (no `://`) becomes `https://` plus what follows its first `@`, with every `:` made a `/`.
// `None` where Java's URI throws or has no host: a space, an underscore in the host, a local
// path.
fn remote_uri(remote_url: &str) -> Option<RemoteUri> {
    let fixed = remote_url.trim_end_matches('/');
    let fixed = fixed.strip_suffix(".git").unwrap_or(fixed);
    if fixed.contains("://") {
        java_uri(fixed)
    } else {
        let address = remove_protocol_prefix(fixed)
            .replace(":/", "/")
            .replace(':', "/");
        java_uri(&format!("https://{address}"))
    }
}

// The part of `java.net.URI`'s parser (RFC 2396) that decides the host and the path of a
// hierarchical URI. Every text it rejects, and every authority it can only read as
// registry-based, gives `None`.
fn java_uri(text: &str) -> Option<RemoteUri> {
    if !legal_uri_characters(text) {
        return None;
    }
    let colon = text.find(':')?;
    let scheme = &text[..colon];
    if scheme.contains(['/', '?', '#']) || !valid_scheme(scheme) {
        return None;
    }
    // Opaque (`scheme:x`): no host.
    let rest = text[colon + 1..].strip_prefix("//")?;
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let host = server_host(&rest[..authority_end])?;
    // Brackets are legal only around an IPv6 host.
    let after = &rest[authority_end..];
    if after.contains(['[', ']']) {
        return None;
    }
    let path_end = after.find(['?', '#']).unwrap_or(after.len());
    let path = &after[..path_end];
    Some(RemoteUri {
        host,
        path: percent_decoded(path)?,
    })
}

// What no part of a URI may hold: controls, spaces, the characters `"<>\^{|}` and the backtick,
// a `%` without two hex digits, a second `#`. Other non-ASCII characters are legal (RFC 2396's
// "other" category).
fn legal_uri_characters(text: &str) -> bool {
    if text.matches('#').count() > 1 {
        return false;
    }
    let bytes = text.as_bytes();
    for (index, character) in text.char_indices() {
        if character == '%' {
            let hex = |at: usize| bytes.get(at).is_some_and(u8::is_ascii_hexdigit);
            if !hex(index + 1) || !hex(index + 2) {
                return false;
            }
        }
        if character.is_control() || character.is_whitespace() || "\"<>\\^`{|}".contains(character)
        {
            return false;
        }
    }
    true
}

fn valid_scheme(scheme: &str) -> bool {
    let mut characters = scheme.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && characters
            .all(|character| character.is_ascii_alphanumeric() || "+-.".contains(character))
}

// URI.parseServer: `[userinfo@]host[:port]`, the host an IPv6 literal, an IPv4 address or a
// hostname. Anything else is a registry-based authority, which has no host. An empty authority
// (`https:///srv/repo`) has none either.
fn server_host(authority: &str) -> Option<String> {
    let server_character = |character: char| {
        character.is_ascii_alphanumeric() || "-_.!~*'();:&=+$,%@[]".contains(character)
    };
    if authority.is_empty() || !authority.chars().all(server_character) {
        return None;
    }
    let (user_info, address) = match authority.find('@') {
        Some(at) => (&authority[..at], &authority[at + 1..]),
        None => ("", authority),
    };
    if user_info.contains(['[', ']']) {
        return None;
    }
    let (host, after) = if address.starts_with('[') {
        let close = address.find(']')?;
        let literal = &address[1..close];
        if literal.is_empty()
            || !literal
                .chars()
                .all(|character| character.is_ascii_hexdigit() || ":.".contains(character))
        {
            return None;
        }
        (&address[..=close], &address[close + 1..])
    } else {
        let end = hostname_end(address)?;
        (&address[..end], &address[end..])
    };
    if let Some(port) = after.strip_prefix(':') {
        if !port.chars().all(|character| character.is_ascii_digit()) {
            return None;
        }
        if !port.is_empty() && port.parse::<i32>().is_err() {
            return None;
        }
    } else if !after.is_empty() {
        return None;
    }
    Some(host.to_string())
}

// URI.parseHostname: labels of letters, digits and inner `-`, separated by single dots; when
// there are several, the last one starts with a letter. A dotted quad is an IPv4 address.
// Returns where the host ends, which must be the end or a `:`.
fn hostname_end(address: &str) -> Option<usize> {
    let bytes = address.as_bytes();
    let end = address.find(':').unwrap_or(address.len());
    if is_ipv4(&address[..end]) {
        return Some(end);
    }
    let mut position = 0;
    let mut last_label = None;
    loop {
        let start = position;
        while position < bytes.len() && bytes[position].is_ascii_alphanumeric() {
            position += 1;
        }
        if position == start {
            break;
        }
        last_label = Some(start);
        let inner = position;
        while position < bytes.len()
            && (bytes[position].is_ascii_alphanumeric() || bytes[position] == b'-')
        {
            position += 1;
        }
        if position > inner && bytes[position - 1] == b'-' {
            return None;
        }
        if position < bytes.len() && bytes[position] == b'.' {
            position += 1;
        } else {
            break;
        }
        if position >= bytes.len() {
            break;
        }
    }
    let last_label = last_label?;
    if position < bytes.len() && bytes[position] != b':' {
        return None;
    }
    if last_label > 0 && !bytes[last_label].is_ascii_alphabetic() {
        return None;
    }
    Some(position)
}

fn is_ipv4(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.len() <= 3
                && part.chars().all(|character| character.is_ascii_digit())
                && part.parse::<u16>().is_ok_and(|value| value <= 255)
        })
}

// URI.getPath decodes `%XX` as UTF-8.
fn percent_decoded(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = std::str::from_utf8(bytes.get(index + 1..index + 3)?).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    Some(String::from_utf8_lossy(&decoded).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(name: &str, url: &str) -> Remote {
        Remote {
            name: name.to_string(),
            url: url.to_string(),
            push_url: None,
        }
    }

    fn server(text: &str) -> GithubServer {
        GithubServer::parse(text).unwrap()
    }

    fn servers() -> Vec<GithubServer> {
        vec![
            GithubServer::github_com(),
            server("https://ghe.example.com/git"),
            GithubServer {
                use_http: None,
                host: "my_host.example.com".to_string(),
                port: None,
                suffix: None,
            },
        ]
    }

    // (server, owner, name) of the one mapping a single remote gives, or `None`.
    fn mapped(url: &str) -> Option<(GithubServer, String, String)> {
        let found = mappings(&[remote("origin", url)], &servers());
        assert!(found.len() <= 1, "{url}");
        found.into_iter().next().map(|mapping| {
            assert_eq!(mapping.remote, "origin");
            assert_eq!(mapping.url, url);
            let coordinates = mapping.coordinates;
            (coordinates.server, coordinates.owner, coordinates.name)
        })
    }

    #[test]
    fn remote_urls_map_as_241() {
        let github = GithubServer::github_com();
        let ghe = server("https://ghe.example.com/git");
        let on = |server: &GithubServer, owner: &str, name: &str| {
            Some((server.clone(), owner.to_string(), name.to_string()))
        };

        assert_eq!(mapped("git@github.com:o/r.git"), on(&github, "o", "r"));
        assert_eq!(mapped("ssh://git@github.com:22/o/r"), on(&github, "o", "r"));
        assert_eq!(mapped("https://github.com/o/r/"), on(&github, "o", "r"));
        assert_eq!(mapped("https://github.com/O/R.git"), on(&github, "O", "R"));
        assert_eq!(mapped("https://GITHUB.com/o/r"), on(&github, "o", "r"));
        // Neither the scheme nor the port is compared.
        assert_eq!(mapped("http://github.com/o/r"), on(&github, "o", "r"));
        assert_eq!(mapped("https://github.com:8443/o/r"), on(&github, "o", "r"));
        assert_eq!(
            mapped("https://user@github.com/o/r.git"),
            on(&github, "o", "r")
        );

        // A GitHub Enterprise behind a path prefix: the owner is the part before the name.
        assert_eq!(
            mapped("https://ghe.example.com/git/team/project.git"),
            on(&ghe, "team", "project")
        );
        assert_eq!(
            mapped("git@ghe.example.com:git/team/project.git"),
            on(&ghe, "team", "project")
        );
        assert_eq!(
            mapped("https://ghe.example.com/GIT/team/project"),
            on(&ghe, "team", "project")
        );
        // The prefix check has no segment boundary: `/git` matches `/gitlab/…`, as in 241.
        assert_eq!(
            mapped("https://ghe.example.com/gitlab/o/r"),
            on(&ghe, "o", "r")
        );
        assert_eq!(mapped("https://ghe.example.com/other/o/r"), None);
        assert_eq!(mapped("https://ghe.example.com/o/r"), None);

        // No mapping: a local path, a host Java's URI does not take as a server (an
        // underscore), a URL with a space, an unknown host, and no owner.
        assert_eq!(mapped("/srv/repo.git"), None);
        assert_eq!(mapped("file:///srv/repo.git"), None);
        assert_eq!(mapped("../repo"), None);
        assert_eq!(mapped("https://my_host.example.com/o/r"), None);
        assert_eq!(mapped("git@my_host.example.com:o/r.git"), None);
        assert_eq!(mapped("https://github.com/o/my repo"), None);
        assert_eq!(mapped("https://gitlab.com/o/r"), None);
        assert_eq!(mapped("git@github.com:r.git"), None);
        assert_eq!(mapped("https://github.com/o/"), None);
        assert_eq!(mapped(""), None);
    }

    #[test]
    fn remotes_on_one_repository_give_one_mapping_and_a_fork_gives_two() {
        let github = GithubServer::github_com();
        let same = mappings(
            &[
                remote("origin", "git@github.com:o/r.git"),
                remote("mirror", "https://github.com/O/R"),
                remote("plain", "http://github.com/o/r/"),
            ],
            &servers(),
        );
        assert_eq!(same.len(), 1);
        assert_eq!(same[0].remote, "origin");
        assert_eq!(same[0].url, "git@github.com:o/r.git");

        let fork = mappings(
            &[
                remote("origin", "git@github.com:me/r.git"),
                remote("upstream", "https://github.com/o/r.git"),
                remote("local", "/srv/r.git"),
            ],
            &servers(),
        );
        let found: Vec<(&str, &str, &str)> = fork
            .iter()
            .map(|mapping| {
                (
                    mapping.remote.as_str(),
                    mapping.coordinates.owner.as_str(),
                    mapping.coordinates.name.as_str(),
                )
            })
            .collect();
        assert_eq!(found, [("origin", "me", "r"), ("upstream", "o", "r")]);
        assert!(fork
            .iter()
            .all(|mapping| mapping.coordinates.server == github));
    }

    #[test]
    fn the_first_server_that_matches_wins() {
        let prefixed = server("https://ghe.example.com/git");
        let plain = server("ghe.example.com");
        let found = mappings(
            &[
                remote("origin", "https://ghe.example.com/git/o/r"),
                remote("other", "https://ghe.example.com/x/y"),
            ],
            &[prefixed.clone(), plain.clone()],
        );
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].coordinates.server, prefixed);
        assert_eq!(found[1].coordinates.server, plain);

        let reversed = mappings(
            &[remote("origin", "https://ghe.example.com/git/o/r")],
            &[plain.clone(), prefixed],
        );
        assert_eq!(reversed[0].coordinates.server, plain);
        assert_eq!(reversed[0].coordinates.owner, "o");
    }

    #[test]
    fn coordinates_compare_ignoring_case_and_scheme() {
        let coordinates = |server: GithubServer, owner: &str, name: &str| RepoCoordinates {
            server,
            owner: owner.to_string(),
            name: name.to_string(),
        };
        let github = GithubServer::github_com();
        assert_eq!(
            coordinates(github.clone(), "Octo", "Repo"),
            coordinates(server("https://github.com"), "octo", "REPO")
        );
        assert_ne!(
            coordinates(github.clone(), "octo", "repo"),
            coordinates(github.clone(), "octo", "repo2")
        );
        assert_ne!(
            coordinates(github, "octo", "repo"),
            coordinates(server("ghe.example.com"), "octo", "repo")
        );
    }

    #[test]
    fn the_web_base_keeps_the_remote_case_and_the_server_prefix() {
        let first = |url: &str, servers: &[GithubServer]| {
            web_base(&mappings(&[remote("origin", url)], servers)[0])
        };
        assert_eq!(
            first("git@github.com:O/R.git", &servers()),
            "https://github.com/O/R"
        );
        assert_eq!(
            first("https://ghe.example.com/git/team/project.git", &servers()),
            "https://ghe.example.com/git/team/project"
        );
        assert_eq!(
            first(
                "ssh://git@ghe.example.com/o/r.git",
                &[server("http://ghe.example.com:8080")]
            ),
            "http://ghe.example.com:8080/o/r"
        );
    }
}
