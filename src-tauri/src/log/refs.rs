use std::cmp::Ordering;
use std::collections::HashMap;

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RefKind {
    Head,
    Local,
    Remote,
    Tag,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitRef {
    pub name: Box<str>,
    pub kind: RefKind,
    pub commit: u32,
}

// `all` is sorted by commit, as it comes out of the log: that way the refs of a commit are found
// without a separate map. `current` and `tracked` hold positions in `all`.
#[derive(Default)]
pub struct Refs {
    pub all: Vec<GitRef>,
    pub current: Option<u32>,
    pub tracked: HashMap<u32, u32>,
}

pub struct RefGroup<'a> {
    pub name: String,
    pub refs: Vec<&'a GitRef>,
}

// A reference read from `%D`. `current` marks the branch that `HEAD -> ` points to.
#[derive(Debug, PartialEq, Eq)]
pub struct Decorated {
    pub name: String,
    pub kind: RefKind,
    pub current: bool,
}

const HEADS: &str = "refs/heads/";
const REMOTES: &str = "refs/remotes/";
const TAGS: &str = "refs/tags/";
const HEAD: &str = "HEAD";
const POINTER: &str = " -> ";
const MASTER: [&str; 2] = ["master", "main"];
const ORIGIN_MASTER: [&str; 2] = ["origin/master", "origin/main"];
// Git puts it in `%D` for commits with grafts or replacements.
const NOT_REFS: [&str; 2] = ["grafted", "replaced"];
const ORIGIN_HEAD: &str = "origin/HEAD";

// `%D` with `--decorate=full`, as read by `GitLogRecord.parseRefNames` and filtered by
// `GitLogUtil.parseRefs`. References that are neither branches nor tags (`refs/stash`,
// `refs/pull/…`) are left out: `RefKind` does not model them.
pub fn parse_decoration(text: &str) -> Vec<Decorated> {
    let mut parsed = Vec::new();
    for item in text.split(", ").filter(|item| !item.is_empty()) {
        match item.split_once(POINTER) {
            Some((from, to)) => {
                parsed.extend(decorated(from.trim(), false));
                parsed.extend(decorated(to.trim(), true));
            }
            None => {
                let raw = item.strip_prefix("tag:").unwrap_or(item).trim();
                parsed.extend(decorated(raw, false));
            }
        }
    }
    parsed
}

fn decorated(raw: &str, current: bool) -> Option<Decorated> {
    if NOT_REFS.contains(&raw) {
        return None;
    }
    let (kind, name) = if raw == HEAD {
        (RefKind::Head, raw)
    } else if let Some(name) = raw.strip_prefix(HEADS) {
        (RefKind::Local, name)
    } else if let Some(name) = raw.strip_prefix(REMOTES) {
        (RefKind::Remote, name)
    } else {
        (RefKind::Tag, raw.strip_prefix(TAGS)?)
    };
    (name != ORIGIN_HEAD).then(|| Decorated {
        name: name.to_string(),
        kind,
        current,
    })
}

// Order of `GitRefManager.GitBranchLayoutComparator`: the best first.
fn layout_rank(git_ref: &GitRef) -> u8 {
    let name = &*git_ref.name;
    match git_ref.kind {
        RefKind::Remote if ORIGIN_MASTER.contains(&name) => 0,
        RefKind::Remote => 1,
        RefKind::Local if MASTER.contains(&name) => 2,
        RefKind::Local => 3,
        RefKind::Tag => 4,
        RefKind::Head => 6,
    }
}

pub fn compare_layout(a: &GitRef, b: &GitRef) -> Ordering {
    layout_rank(a)
        .cmp(&layout_rank(b))
        .then_with(|| a.name.cmp(&b.name))
}

// The one that best represents the commits of a head, for its color and its order.
pub fn best_ref<'a>(refs_of_head: &[&'a GitRef]) -> Option<&'a GitRef> {
    refs_of_head
        .iter()
        .copied()
        .min_by(|a, b| compare_layout(a, b))
}

// Order of the labels of `GitRefManager.GitLabelComparator`.
fn label_rank(git_ref: &GitRef, current: Option<&str>) -> u8 {
    let name = &*git_ref.name;
    match git_ref.kind {
        RefKind::Head => 0,
        RefKind::Local if Some(name) == current => 1,
        RefKind::Local if MASTER.contains(&name) => 2,
        RefKind::Remote if ORIGIN_MASTER.contains(&name) => 3,
        RefKind::Local => 4,
        RefKind::Remote => 5,
        RefKind::Tag => 6,
    }
}

impl Refs {
    pub fn current_name(&self) -> Option<&str> {
        self.current.map(|index| &*self.all[index as usize].name)
    }

    // The references of a commit, in the order in which they are shown.
    pub fn of_commit(&self, commit: u32) -> Vec<&GitRef> {
        let start = self.all.partition_point(|git_ref| git_ref.commit < commit);
        let end = self.all.partition_point(|git_ref| git_ref.commit <= commit);
        let mut found: Vec<_> = self.all[start..end].iter().collect();
        self.sort_labels(&mut found);
        found
    }

    pub fn sort_labels(&self, refs: &mut [&GitRef]) {
        let current = self.current_name();
        refs.sort_by(|a, b| {
            label_rank(a, current)
                .cmp(&label_rank(b, current))
                .then_with(|| a.name.cmp(&b.name))
        });
    }

    // The position in `all` of a reference that comes from `all`.
    fn index_of(&self, git_ref: &GitRef) -> Option<u32> {
        let size = std::mem::size_of::<GitRef>();
        let offset = (git_ref as *const GitRef as usize).checked_sub(self.all.as_ptr() as usize)?;
        (offset / size < self.all.len() && offset % size == 0).then_some((offset / size) as u32)
    }

    // `upstreams`: local branch and the remote one it tracks, according to git's configuration.
    // Without it, `<remote>/<branch>`, and with several, `origin` wins and then the name (git does
    // not order the remotes).
    pub fn track(&mut self, upstreams: &HashMap<String, String>) {
        let mut remotes: HashMap<&str, u32> = HashMap::new();
        let mut by_branch: HashMap<&str, Vec<u32>> = HashMap::new();
        for (index, git_ref) in self.all.iter().enumerate() {
            if git_ref.kind != RefKind::Remote {
                continue;
            }
            remotes.insert(&git_ref.name, index as u32);
            if let Some((_, branch)) = git_ref.name.split_once('/') {
                by_branch.entry(branch).or_default().push(index as u32);
            }
        }
        let mut tracked = HashMap::new();
        for (index, git_ref) in self.all.iter().enumerate() {
            if git_ref.kind != RefKind::Local {
                continue;
            }
            let configured = upstreams
                .get(&*git_ref.name)
                .and_then(|upstream| remotes.get(upstream.as_str()));
            let found = configured.copied().or_else(|| {
                let candidates = by_branch.get(&*git_ref.name)?;
                candidates.iter().copied().min_by_key(|candidate| {
                    let name = &*self.all[*candidate as usize].name;
                    (!name.starts_with("origin/"), name)
                })
            });
            if let Some(remote) = found {
                tracked.insert(index as u32, remote);
            }
        }
        self.tracked = tracked;
    }
}

fn remote_name(git_ref: &GitRef) -> &str {
    git_ref
        .name
        .split_once('/')
        .map_or(&git_ref.name, |(remote, _)| remote)
}

// `GitRefManager.groupForTable` y `SimpleRefGroup.buildGroups`.
pub fn groups<'a>(
    refs_of_commit: &[&'a GitRef],
    refs: &Refs,
    compact: bool,
    show_tag_names: bool,
) -> Vec<RefGroup<'a>> {
    if refs_of_commit.is_empty() {
        return Vec::new();
    }
    let mut sorted = refs_of_commit.to_vec();
    refs.sort_labels(&mut sorted);
    let (heads, mut rest): (Vec<_>, Vec<_>) = sorted
        .into_iter()
        .partition(|git_ref| git_ref.kind == RefKind::Head);

    let mut leading = Vec::new();
    let mut tracked_groups = Vec::new();
    for local in rest.iter().filter(|git_ref| git_ref.kind == RefKind::Local) {
        let remote = refs
            .index_of(local)
            .and_then(|index| refs.tracked.get(&index))
            .map(|remote| &refs.all[*remote as usize])
            .and_then(|remote| {
                rest.iter()
                    .find(|candidate| std::ptr::eq(**candidate, remote))
            });
        if let Some(remote) = remote {
            tracked_groups.push(RefGroup {
                name: format!("{} & {}", remote_name(remote), local.name),
                refs: vec![*local, *remote],
            });
        }
    }
    for group in &tracked_groups {
        rest.retain(|git_ref| {
            !group
                .refs
                .iter()
                .any(|taken| std::ptr::eq(*taken, *git_ref))
        });
    }
    let current = refs.current_name().and_then(|name| {
        let position = rest
            .iter()
            .position(|git_ref| git_ref.kind == RefKind::Local && &*git_ref.name == name)?;
        let branch = rest.remove(position);
        Some(RefGroup {
            name: name.to_string(),
            refs: vec![branch],
        })
    });
    leading.extend(current);
    leading.extend(tracked_groups);

    let mut result = build_groups(leading, rest, compact, show_tag_names);
    if heads.is_empty() {
        return result;
    }
    if refs.current.is_none() {
        result.insert(
            0,
            RefGroup {
                name: "!".to_string(),
                refs: heads,
            },
        );
    } else if let Some(first) = result.first_mut() {
        first.refs.splice(0..0, heads);
    } else {
        result.push(RefGroup {
            name: String::new(),
            refs: heads,
        });
    }
    result
}

fn build_groups<'a>(
    groups: Vec<RefGroup<'a>>,
    rest: Vec<&'a GitRef>,
    compact: bool,
    show_tag_names: bool,
) -> Vec<RefGroup<'a>> {
    if rest.is_empty() {
        return groups;
    }
    // By kind, in the order in which the first of each one appears.
    let mut by_kind: Vec<(RefKind, Vec<&GitRef>)> = Vec::new();
    for git_ref in rest {
        match by_kind.iter_mut().find(|(kind, _)| *kind == git_ref.kind) {
            Some((_, of_kind)) => of_kind.push(git_ref),
            None => by_kind.push((git_ref.kind, vec![git_ref])),
        }
    }
    let is_branch = |kind: RefKind| kind != RefKind::Tag;
    if compact {
        if groups.is_empty() {
            let first = by_kind[0].1[0];
            let name = if is_branch(first.kind) || show_tag_names {
                first.name.to_string()
            } else {
                String::new()
            };
            let refs = by_kind.into_iter().flat_map(|(_, refs)| refs).collect();
            return vec![RefGroup { name, refs }];
        }
        let name = groups[0].name.clone();
        let refs = groups
            .into_iter()
            .flat_map(|group| group.refs)
            .chain(by_kind.into_iter().flat_map(|(_, refs)| refs))
            .collect();
        return vec![RefGroup { name, refs }];
    }
    let mut result = groups;
    for (kind, refs) in by_kind {
        if is_branch(kind) {
            result.extend(refs.into_iter().map(|git_ref| RefGroup {
                name: git_ref.name.to_string(),
                refs: vec![git_ref],
            }));
        } else {
            let name = if show_tag_names {
                refs[0].name.to_string()
            } else {
                String::new()
            };
            result.push(RefGroup { name, refs });
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_ref(name: &str, kind: RefKind) -> GitRef {
        GitRef {
            name: name.into(),
            kind,
            commit: 0,
        }
    }

    fn names<'a>(refs: &[&'a GitRef]) -> Vec<&'a str> {
        refs.iter().map(|git_ref| &*git_ref.name).collect()
    }

    #[test]
    fn decorate_separates_head_and_branch() {
        let parsed = parse_decoration("HEAD -> refs/heads/x");
        assert_eq!(
            parsed,
            [
                Decorated {
                    name: "HEAD".into(),
                    kind: RefKind::Head,
                    current: false
                },
                Decorated {
                    name: "x".into(),
                    kind: RefKind::Local,
                    current: true
                },
            ]
        );
    }

    #[test]
    fn decorate_removes_origin_head_and_grafted() {
        let parsed = parse_decoration(
            "HEAD -> refs/heads/main, refs/remotes/origin/HEAD, refs/remotes/origin/main, grafted, replaced",
        );
        let names: Vec<_> = parsed.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, ["HEAD", "main", "origin/main"]);
        assert_eq!(parsed[2].kind, RefKind::Remote);
    }

    #[test]
    fn decorate_recognizes_tag() {
        let parsed = parse_decoration("tag: refs/tags/v1.0, refs/heads/rel/1");
        assert_eq!(
            parsed,
            [
                Decorated {
                    name: "v1.0".into(),
                    kind: RefKind::Tag,
                    current: false
                },
                Decorated {
                    name: "rel/1".into(),
                    kind: RefKind::Local,
                    current: false
                },
            ]
        );
    }

    #[test]
    fn decorate_detached_head_and_empty() {
        let parsed = parse_decoration("HEAD, refs/tags/t");
        assert_eq!(parsed[0].kind, RefKind::Head);
        assert!(!parsed[0].current);
        assert!(parse_decoration("").is_empty());
    }

    #[test]
    fn decorate_discards_references_foreign_to_the_log() {
        let parsed = parse_decoration("refs/stash, refs/pull/1/head, refs/heads/a");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "a");
    }

    #[test]
    fn best_ref_prefers_origin_main() {
        let refs = [
            git_ref("main", RefKind::Local),
            git_ref("origin/main", RefKind::Remote),
            git_ref("HEAD", RefKind::Head),
        ];
        let at_head: Vec<_> = refs.iter().collect();
        assert_eq!(&*best_ref(&at_head).unwrap().name, "origin/main");
    }

    #[test]
    fn best_ref_full_order() {
        let refs = [
            git_ref("zeta", RefKind::Tag),
            git_ref("HEAD", RefKind::Head),
            git_ref("other", RefKind::Local),
            git_ref("master", RefKind::Local),
            git_ref("upstream/dev", RefKind::Remote),
            git_ref("origin/master", RefKind::Remote),
            git_ref("origin/main", RefKind::Remote),
            git_ref("origin/dev", RefKind::Remote),
        ];
        let mut rest: Vec<&GitRef> = refs.iter().collect();
        let mut order = Vec::new();
        while let Some(best) = best_ref(&rest) {
            order.push(&*best.name);
            rest.retain(|git_ref| !std::ptr::eq(*git_ref, best));
        }
        assert_eq!(
            order,
            [
                "origin/main",
                "origin/master",
                "origin/dev",
                "upstream/dev",
                "master",
                "other",
                "zeta",
                "HEAD"
            ]
        );
        assert!(best_ref(&[]).is_none());
    }

    // master and origin/master on the same commit, with HEAD on master.
    fn master_refs() -> Refs {
        Refs {
            all: vec![
                git_ref("HEAD", RefKind::Head),
                git_ref("master", RefKind::Local),
                git_ref("origin/master", RefKind::Remote),
                git_ref("v1", RefKind::Tag),
                git_ref("other", RefKind::Local),
            ],
            current: Some(1),
            tracked: HashMap::from([(1, 2)]),
        }
    }

    #[test]
    fn groups_join_local_and_tracked_remote() {
        let refs = master_refs();
        let at_commit = [&refs.all[2], &refs.all[1], &refs.all[0]];

        let groups = groups(&at_commit, &refs, true, false);

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "origin & master");
        assert_eq!(names(&groups[0].refs), ["HEAD", "master", "origin/master"]);
    }

    #[test]
    fn groups_detached_head_gets_exclamation_mark() {
        let mut refs = master_refs();
        refs.current = None;
        refs.tracked.clear();
        let at_commit = [&refs.all[0], &refs.all[4]];

        let groups = groups(&at_commit, &refs, true, false);

        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].name, "!");
        assert_eq!(names(&groups[0].refs), ["HEAD"]);
        assert_eq!(groups[1].name, "other");
    }

    #[test]
    fn groups_without_compacting_separate_each_branch_and_join_the_tags() {
        let refs = master_refs();
        let at_commit = [&refs.all[3], &refs.all[4], &refs.all[1], &refs.all[2]];

        let groups = groups(&at_commit, &refs, false, false);

        let summary: Vec<_> = groups
            .iter()
            .map(|group| (group.name.as_str(), names(&group.refs)))
            .collect();
        assert_eq!(
            summary,
            [
                ("origin & master", vec!["master", "origin/master"]),
                ("other", vec!["other"]),
                ("", vec!["v1"]),
            ]
        );
    }

    #[test]
    fn groups_with_tag_names() {
        let refs = master_refs();
        let at_commit = [&refs.all[3]];
        assert_eq!(groups(&at_commit, &refs, true, true)[0].name, "v1");
        assert_eq!(groups(&at_commit, &refs, true, false)[0].name, "");
    }

    #[test]
    fn compact_groups_are_named_after_the_first() {
        let refs = master_refs();
        let at_commit = [&refs.all[4], &refs.all[3]];

        let groups = groups(&at_commit, &refs, true, false);

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "other");
        assert_eq!(names(&groups[0].refs), ["other", "v1"]);
    }

    #[test]
    fn groups_the_current_branch_goes_alone_and_first() {
        let mut refs = master_refs();
        refs.tracked.clear();
        let at_commit = [&refs.all[4], &refs.all[1]];

        let groups = groups(&at_commit, &refs, false, false);

        let summary: Vec<_> = groups.iter().map(|group| group.name.as_str()).collect();
        assert_eq!(summary, ["master", "other"]);
    }

    #[test]
    fn tracking_by_configuration_and_by_name() {
        let mut refs = Refs {
            all: vec![
                git_ref("a", RefKind::Local),
                git_ref("b", RefKind::Local),
                git_ref("c", RefKind::Local),
                git_ref("fork/c", RefKind::Remote),
                git_ref("origin/c", RefKind::Remote),
                git_ref("origin/x", RefKind::Remote),
            ],
            ..Refs::default()
        };

        refs.track(&HashMap::from([("a".to_string(), "origin/x".to_string())]));

        assert_eq!(refs.tracked.get(&0), Some(&5));
        assert_eq!(refs.tracked.get(&1), None);
        // Without configuration, `<remote>/<branch>`; with two candidates origin wins.
        assert_eq!(refs.tracked.get(&2), Some(&4));
    }

    #[test]
    fn references_of_a_commit() {
        let mut refs = master_refs();
        for (index, git_ref) in refs.all.iter_mut().enumerate() {
            git_ref.commit = if index < 3 { 4 } else { 9 };
        }
        assert_eq!(
            names(&refs.of_commit(4)),
            ["HEAD", "master", "origin/master"]
        );
        assert_eq!(names(&refs.of_commit(9)), ["other", "v1"]);
        assert!(refs.of_commit(5).is_empty());
    }
}
