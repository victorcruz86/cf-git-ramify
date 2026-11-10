// `Open on GitHub` and `Copy Link to GitHub Repository`: the URLs of a commit and of a file, one
// per GitHub repository the open repository's remotes point to. Ports of GHOpenInBrowserActionGroup
// (the URL formats), HostedGitRepositoryReferenceUtil and HostedGitRepositoryReferenceActionGroup
// (which reference each place builds, and when there is none), at idea/241.14494.240.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::mapping::{mappings, web_base};
use super::server::GithubServer;
use crate::diff;
use crate::git::{self, GitError};
use crate::history::NO_SIGNATURES;
use crate::remotes;

// GithubBundle's notification, `GitB:1560`.
pub const CANNOT_OPEN: &str = "Cannot open in browser";
// `GitB:1562`.
const CANNOT_RETRIEVE: &str = "Cannot retrieve latest revision for file";

// What a place asks a link for. A file is relative to the repository root; `lines` are 0-based,
// the first and the last, as the editor's selection gives them (they can be `-1` after 241's
// column-0 rule, see `file_url`). `revision` is the commit of a revision tab; without it the
// file's last commit in HEAD is used.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LinkTarget {
    Commit {
        hash: String,
    },
    File {
        path: String,
        #[serde(default)]
        lines: Option<(i64, i64)>,
        #[serde(default)]
        revision: Option<String>,
    },
}

// One GitHub repository's link: `name` is what the submenu shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Reference {
    pub name: String,
    pub url: String,
}

// `<base>/commit/<fullHash>`.
pub fn commit_url(base: &str, hash: &str) -> String {
    format!("{base}/commit/{hash}")
}

// `<base>/blob/<revision>/<encodePath(path)>`, with `#L<first+1>` and `-L<last+1>` when the last
// line is another one. The lines are 0-based in and 1-based out, and there is no check on them:
// 241's end-line rule gives `#Lk-L(k-1)` for a caret at column 0.
pub fn file_url(base: &str, revision: &str, path: &str, lines: Option<(i64, i64)>) -> String {
    let mut url = format!("{base}/blob/{revision}/{}", encode_path(path));
    if let Some((first, last)) = lines {
        url.push_str(&format!("#L{}", first + 1));
        if last != first {
            url.push_str(&format!("-L{}", last + 1));
        }
    }
    url
}

// `URLUtil.encodePath`: `new URI(null, null, path, null, null).toASCIIString()`. What the path of a
// URI allows stays (unreserved and `;:@&=+$,` and `/`); every other byte, the UTF-8 of a non-ASCII
// character included, is `%XX`.
fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for byte in path.bytes() {
        let kept = byte.is_ascii_alphanumeric() || b"-_.!~*'();:@&=+$,/".contains(&byte);
        if kept {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

// `GitHistoryUtils.getCurrentRevision(project, path, "HEAD")`: the last commit in HEAD that touched
// the path, which is not HEAD. `None` when none did (a file that is untracked, added or ignored, or
// a repository with no commit).
pub fn last_revision_of(repo: &Path, path: &str) -> Result<Option<String>, GitError> {
    if !git::head_exists(repo)? {
        return Ok(None);
    }
    let args = git::exact(&[
        NO_SIGNATURES[0],
        NO_SIGNATURES[1],
        "--literal-pathspecs",
        "log",
        "-n1",
        "--format=%H",
        "HEAD",
        "--",
        path,
    ]);
    let output = git::run(repo, &args)?;
    let hash = String::from_utf8_lossy(&output).trim().to_string();
    if hash.is_empty() {
        return Ok(None);
    }
    diff::check_hash(&hash)?;
    Ok(Some(hash))
}

// One reference per GitHub repository of the remotes. The name is the web address with `_` as a
// space, as `HostedGitRepositoryReferenceUtil` writes it. It is empty when no remote points to a
// known server and, for a file, for the repository root and for one with no commit in HEAD.
pub fn references(
    repo: &Path,
    servers: &[GithubServer],
    target: &LinkTarget,
) -> Result<Vec<Reference>, GitError> {
    let remotes = remotes::list(repo)?;
    let found = mappings(&remotes, servers);
    if found.is_empty() {
        return Ok(Vec::new());
    }
    let url_of: Box<dyn Fn(&str) -> String> = match target {
        LinkTarget::Commit { hash } => {
            diff::check_hash(hash)?;
            let hash = hash.clone();
            Box::new(move |base| commit_url(base, &hash))
        }
        LinkTarget::File {
            path,
            lines,
            revision,
        } => {
            if matches!(path.trim_matches('/'), "" | ".") {
                return Ok(Vec::new());
            }
            let revision = match revision {
                Some(revision) => {
                    diff::check_hash(revision)?;
                    revision.clone()
                }
                None => match last_revision_of(repo, path)
                    .map_err(|_| GitError::Io(CANNOT_RETRIEVE.to_string()))?
                {
                    Some(revision) => revision,
                    None => return Ok(Vec::new()),
                },
            };
            let (path, lines) = (path.clone(), *lines);
            Box::new(move |base| file_url(base, &revision, &path, lines))
        }
    };
    let mut references: Vec<Reference> = Vec::new();
    for mapping in &found {
        let base = web_base(mapping);
        let reference = Reference {
            url: url_of(&base),
            name: base.replace('_', " "),
        };
        if !references.contains(&reference) {
            references.push(reference);
        }
    }
    Ok(references)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::test_support::TempRepo;

    const BASE: &str = "https://github.com/octo/repo";
    const HASH: &str = "0123456789abcdef0123456789abcdef01234567";

    fn servers() -> Vec<GithubServer> {
        vec![GithubServer::github_com()]
    }

    fn file(path: &str) -> LinkTarget {
        LinkTarget::File {
            path: path.to_string(),
            lines: None,
            revision: None,
        }
    }

    fn with_origin(url: &str) -> TempRepo {
        let repo = TempRepo::new();
        repo.git(&["remote", "add", "origin", url]);
        repo
    }

    #[test]
    fn urls_follow_241() {
        assert_eq!(commit_url(BASE, HASH), format!("{BASE}/commit/{HASH}"));
        assert_eq!(
            file_url(BASE, HASH, "src/dir with space/caf\u{e9}.txt", None),
            format!("{BASE}/blob/{HASH}/src/dir%20with%20space/caf%C3%A9.txt")
        );
        // `%`, `#` and `?` of a name are never taken for the URL's own.
        assert_eq!(
            file_url(BASE, HASH, "a%b#c?d.txt", None),
            format!("{BASE}/blob/{HASH}/a%25b%23c%3Fd.txt")
        );
        assert_eq!(
            file_url(BASE, HASH, "a.txt", Some((0, 0))),
            format!("{BASE}/blob/{HASH}/a.txt#L1")
        );
        assert_eq!(
            file_url(BASE, HASH, "a.txt", Some((4, 9))),
            format!("{BASE}/blob/{HASH}/a.txt#L5-L10")
        );
        // A directory is the same URL, which GitHub redirects.
        assert_eq!(
            file_url(BASE, HASH, "src/dir", None),
            format!("{BASE}/blob/{HASH}/src/dir")
        );
        // The caret-at-column-0 quirk reaches here as `last = first - 1`.
        assert_eq!(
            file_url(BASE, HASH, "a.txt", Some((6, 5))),
            format!("{BASE}/blob/{HASH}/a.txt#L7-L6")
        );
    }

    #[test]
    fn a_file_url_uses_the_last_commit_that_touched_it_not_head() {
        let repo = with_origin("git@github.com:octo/repo.git");
        let touched = repo.commit("a.txt", b"one\n", "touch a");
        let head = repo.commit("b.txt", b"two\n", "touch b");
        assert_ne!(touched, head);
        assert_eq!(
            last_revision_of(repo.path(), "a.txt").unwrap(),
            Some(touched.clone())
        );
        let found = references(repo.path(), &servers(), &file("a.txt")).unwrap();
        assert_eq!(
            found,
            [Reference {
                name: BASE.to_string(),
                url: format!("{BASE}/blob/{touched}/a.txt"),
            }]
        );
        // A path that looks like an option is a path.
        assert_eq!(last_revision_of(repo.path(), "--all").unwrap(), None);
        // The commit's URL names the commit given.
        let commit = LinkTarget::Commit { hash: head.clone() };
        assert_eq!(
            references(repo.path(), &servers(), &commit).unwrap()[0].url,
            format!("{BASE}/commit/{head}")
        );
        // A revision tab names its own revision.
        let tab = LinkTarget::File {
            path: "a.txt".to_string(),
            lines: Some((1, 2)),
            revision: Some(head.clone()),
        };
        assert_eq!(
            references(repo.path(), &servers(), &tab).unwrap()[0].url,
            format!("{BASE}/blob/{head}/a.txt#L2-L3")
        );
    }

    #[test]
    fn no_reference_for_untracked_added_ignored_or_root() {
        let repo = with_origin("git@github.com:octo/repo.git");
        // No commit yet: everything is untracked or added.
        fs::write(repo.path().join("early.txt"), b"x").unwrap();
        repo.git(&["add", "early.txt"]);
        assert!(references(repo.path(), &servers(), &file("early.txt"))
            .unwrap()
            .is_empty());
        repo.commit(".gitignore", b"ignored.txt\n", "ignore");
        fs::write(repo.path().join("untracked.txt"), b"x").unwrap();
        fs::write(repo.path().join("ignored.txt"), b"x").unwrap();
        fs::write(repo.path().join("added.txt"), b"x").unwrap();
        repo.git(&["add", "added.txt"]);
        for path in ["untracked.txt", "ignored.txt", "added.txt", "", ".", "/"] {
            assert!(
                references(repo.path(), &servers(), &file(path))
                    .unwrap()
                    .is_empty(),
                "{path:?}"
            );
        }
        assert_eq!(
            references(repo.path(), &servers(), &file(".gitignore"))
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn two_github_remotes_give_a_submenu_named_by_url() {
        let repo = with_origin("git@github.com:octo/repo.git");
        repo.git(&["remote", "add", "fork", "https://github.com/me/my_fork.git"]);
        let hash = repo.commit("a.txt", b"x", "one");
        let found = references(
            repo.path(),
            &servers(),
            &LinkTarget::Commit { hash: hash.clone() },
        )
        .unwrap();
        assert_eq!(
            found,
            [
                Reference {
                    name: BASE.to_string(),
                    url: format!("{BASE}/commit/{hash}"),
                },
                Reference {
                    name: "https://github.com/me/my fork".to_string(),
                    url: format!("https://github.com/me/my_fork/commit/{hash}"),
                },
            ]
        );
    }

    #[test]
    fn no_github_remote_gives_no_reference() {
        let repo = with_origin("https://gitlab.com/octo/repo.git");
        let hash = repo.commit("a.txt", b"x", "one");
        assert!(
            references(repo.path(), &servers(), &LinkTarget::Commit { hash })
                .unwrap()
                .is_empty()
        );
        assert!(references(repo.path(), &servers(), &file("a.txt"))
            .unwrap()
            .is_empty());
        // A server of an account makes its remotes count.
        let ghe = GithubServer::parse("ghe.example.com").unwrap();
        repo.git(&[
            "remote",
            "add",
            "work",
            "https://ghe.example.com/team/proj.git",
        ]);
        let found = references(
            repo.path(),
            &[servers(), vec![ghe]].concat(),
            &file("a.txt"),
        )
        .unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0]
            .url
            .starts_with("https://ghe.example.com/team/proj/blob/"));
    }

    #[test]
    fn a_hash_that_could_be_an_option_is_refused() {
        let repo = with_origin("git@github.com:octo/repo.git");
        repo.commit("a.txt", b"x", "one");
        let target = LinkTarget::Commit {
            hash: "--upload-pack=x".to_string(),
        };
        assert!(references(repo.path(), &servers(), &target).is_err());
    }
}
