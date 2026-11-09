use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::git::GitError;
use crate::log::collapse::CollapsedGraph;
use crate::log::graph::LinearGraph;
use crate::log::refs::{GitRef, RefKind};
use crate::log::store::{CommitStore, Person};

// `VcsLogFilterObject.ME`: the git user of this repository.
pub const ME: &str = "*";
const HEAD: &str = "HEAD";
// `GitLogUtil.LOG_ALL`.
const ALL_REFS: [&str; 4] = [HEAD, "--branches", "--remotes", "--tags"];
// `VcsLogUtil.isRegexp`.
const REGEX_CHARS: &str = r"()[]{}.*?+^$\|";
// `UserNameRegex.EXTENDED_REGEX_CHARS`.
const AUTHOR_REGEX_CHARS: &str = ".^$*+-?()[]{}|";
// `VcsLogUtil.HASH_REGEX`.
const HASH_LENGTHS: std::ops::RangeInclusive<usize> = 7..=40;
const SHORT_HASH_LENGTH: usize = 8;
const FULL_HASH_LENGTH: usize = 40;

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextFilter {
    pub text: String,
    #[serde(default)]
    pub regex: bool,
    #[serde(default)]
    pub match_case: bool,
}

// `branches` are patterns from `fromBranchPatterns` and `revisions`, full hashes whose history is
// shown. `hashes`, in contrast, shows only those commits. `users` are names or `ME`, and `since`
// and `until` are in seconds since 1970.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LogFilter {
    pub branches: Vec<String>,
    pub users: Vec<String>,
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub paths: Vec<String>,
    pub text: Option<TextFilter>,
    pub hashes: Vec<String>,
    pub revisions: Vec<String>,
    pub ranges: Vec<(String, String)>,
}

impl LogFilter {
    fn text_filter(&self) -> Option<&TextFilter> {
        self.text
            .as_ref()
            .filter(|text| !text.text.trim().is_empty())
    }

    fn branch_refs<'a>(&self, store: &'a CommitStore) -> Vec<&'a GitRef> {
        if self.branches.is_empty() {
            return Vec::new();
        }
        let refs = store.refs().all.iter();
        let branches = refs.filter(|git_ref| git_ref.kind != RefKind::Tag);
        let existing: HashSet<&str> = branches.clone().map(|branch| &*branch.name).collect();
        let matcher = BranchMatcher::new(&self.branches, &existing);
        branches
            .filter(|branch| matcher.matches(&branch.name))
            .collect()
    }

    // `VcsLogFilterCollection.getDetailsFilters`: what has to be asked of git.
    fn has_details(&self) -> bool {
        self.text_filter().is_some()
            || !self.users.is_empty()
            || !self.paths.is_empty()
            || self.since.is_some()
            || self.until.is_some()
    }
}

// The names of the branches (and `HEAD`) the Branch filter values match, as the file history passes
// them to git (`GitLogProvider.getBranchLikeFilterParameters`).
pub fn matching_branches(store: &CommitStore, values: &[String]) -> Vec<String> {
    let filter = LogFilter {
        branches: values.to_vec(),
        ..LogFilter::default()
    };
    let mut seen = HashSet::new();
    filter
        .branch_refs(store)
        .into_iter()
        .map(|git_ref| git_ref.name.to_string())
        .filter(|name| seen.insert(name.clone()))
        .collect()
}

// `VcsLogBranchFilterImpl`, created as `fromBranchPatterns`. A name that exists is taken as is
// before looking at the dash: a branch can be called `-x`.
#[derive(Default)]
struct BranchMatcher {
    names: Vec<String>,
    patterns: Vec<Regex>,
    excluded_names: Vec<String>,
    excluded_patterns: Vec<Regex>,
}

impl BranchMatcher {
    fn new(values: &[String], existing: &HashSet<&str>) -> Self {
        let mut matcher = Self::default();
        for value in values {
            if existing.contains(value.as_str()) {
                matcher.names.push(value.clone());
                continue;
            }
            let (text, excluded) = match value.strip_prefix('-') {
                Some(text) => (text, true),
                None => (value.as_str(), false),
            };
            let regex = (!existing.contains(text) && has_regex_chars(text))
                .then(|| Regex::new(&format!("^(?:{text})$")).ok())
                .flatten();
            match (regex, excluded) {
                (Some(regex), false) => matcher.patterns.push(regex),
                (Some(regex), true) => matcher.excluded_patterns.push(regex),
                (None, false) => matcher.names.push(text.to_string()),
                (None, true) => matcher.excluded_names.push(text.to_string()),
            }
        }
        matcher
    }

    fn found(name: &str, names: &[String], patterns: &[Regex]) -> bool {
        names.iter().any(|known| known == name) || patterns.iter().any(|regex| regex.is_match(name))
    }

    fn matches(&self, name: &str) -> bool {
        let included = self.names.is_empty() && self.patterns.is_empty()
            || Self::found(name, &self.names, &self.patterns);
        included && !Self::found(name, &self.excluded_names, &self.excluded_patterns)
    }
}

// One bit per commit of the store.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BitVec {
    words: Vec<u64>,
}

impl BitVec {
    pub fn new(len: usize) -> Self {
        Self {
            words: vec![0; len.div_ceil(64)],
        }
    }

    pub fn set(&mut self, index: u32) {
        self.words[index as usize / 64] |= 1 << (index % 64);
    }

    pub fn get(&self, index: u32) -> bool {
        self.words[index as usize / 64] & (1 << (index % 64)) != 0
    }

    pub fn count(&self) -> u32 {
        self.words.iter().map(|word| word.count_ones()).sum()
    }

    fn intersect(&mut self, other: &BitVec) {
        for (word, other) in self.words.iter_mut().zip(&other.words) {
            *word &= other;
        }
    }
}

// What a filter lets through, like the two sets of `createVisibleGraph`.
#[derive(Clone, Debug, Default)]
pub enum Selection {
    #[default]
    All,
    // What is reachable from the chosen branches: the graph is still the real one.
    Heads(Arc<BitVec>),
    // What matches: `FilteredController` hides the rest and joins what remains.
    Commits(Arc<BitVec>),
}

fn escape_backslashes(text: &str) -> String {
    text.replace('\\', r"\\")
}

// `UserNameRegex.EXTENDED_INSTANCE`.
fn author_regex(user: &str) -> String {
    let mut regex = String::from("^");
    for c in escape_backslashes(user).chars() {
        if AUTHOR_REGEX_CHARS.contains(c) {
            regex.push('\\');
        }
        regex.push(c);
    }
    regex.push('$');
    regex
}

// `VcsLogFilterObject.fromPattern`: Regex only counts if there are metacharacters and it
// compiles.
fn is_regex(text: &TextFilter) -> bool {
    text.regex && has_regex_chars(&text.text) && Regex::new(&text.text).is_ok()
}

fn has_regex_chars(text: &str) -> bool {
    text.contains(|c| REGEX_CHARS.contains(c))
}

// `GitLogProvider.getCommitsMatchingFilter`, except that the revisions go at the end: after
// `--end-of-options` none can be an option, and `--branches` and company would no longer be one.
// `users` must arrive resolved with `resolve_users`.
pub fn git_args(filter: &LogFilter) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(since) = filter.since {
        args.push(format!("--after={since}"));
    }
    if let Some(until) = filter.until {
        args.push(format!("--before={until}"));
    }
    let text = filter.text_filter();
    // Without text, git treats the author as a regex.
    let regexp = text.is_none_or(is_regex);
    if let Some(text) = text {
        args.push(format!("--grep={}", text.text));
    }
    args.push(
        if regexp {
            "--extended-regexp"
        } else {
            "--fixed-strings"
        }
        .to_string(),
    );
    if !text.is_some_and(|text| text.match_case) {
        args.push("--regexp-ignore-case".to_string());
    }
    if regexp && !filter.users.is_empty() {
        let authors: Vec<String> = filter.users.iter().map(|user| author_regex(user)).collect();
        args.push(format!("--author={}", authors.join("|")));
    } else if !regexp {
        for user in &filter.users {
            args.push(format!("--author={}", escape_backslashes(user)));
        }
    }
    if !filter.paths.is_empty() {
        args.push("--full-history".to_string());
        args.push("--simplify-merges".to_string());
    }
    if filter.branches.is_empty() && filter.revisions.is_empty() && filter.ranges.is_empty() {
        args.extend(ALL_REFS.map(String::from));
    } else {
        args.push("--end-of-options".to_string());
        args.extend(filter.branches.iter().cloned());
        args.extend(filter.revisions.iter().cloned());
        for (exclusive, inclusive) in &filter.ranges {
            args.push(format!("{exclusive}..{inclusive}"));
        }
    }
    args.push("--".to_string());
    args.extend(filter.paths.iter().cloned());
    args
}

pub fn reachable(store: &CommitStore, heads: &[u32]) -> BitVec {
    let mut seen = BitVec::new(store.len() as usize);
    let mut pending = heads.to_vec();
    while let Some(commit) = pending.pop() {
        if !seen.get(commit) {
            seen.set(commit);
            pending.extend_from_slice(&store.get(commit).parents);
        }
    }
    seen
}

// `DottedFilterEdgesGenerator.update` over the whole graph, and so without arrows: its case is
// that the edge leaves the stretch being updated.
fn dotted_rows(graph: &LinearGraph, visible: &[bool]) -> Vec<(u32, u32)> {
    let shown = |row: u32| visible[row as usize];
    let mut numbers = vec![0; visible.len()];
    let mut edges = Vec::new();
    for row in 0..graph.len() {
        let ups = graph.ups.of(row).iter().map(|edge| edge.up);
        let mut nearly = i32::MIN;
        if shown(row) {
            let mut adjacent = i32::MIN;
            for up in ups {
                let number = numbers[up as usize];
                if shown(up) {
                    adjacent = adjacent.max(number);
                } else {
                    nearly = nearly.max(number);
                }
            }
            if nearly == adjacent || nearly == i32::MIN {
                numbers[row as usize] = adjacent;
            } else {
                edges.push((nearly as u32, row));
                numbers[row as usize] = nearly;
            }
        } else {
            for up in ups {
                let number = if shown(up) {
                    up as i32
                } else {
                    numbers[up as usize]
                };
                nearly = nearly.max(number);
            }
            numbers[row as usize] = nearly;
        }
    }
    numbers.fill(i32::MAX);
    for row in (0..graph.len()).rev() {
        let downs = graph.downs.of(row).iter().map(|edge| edge.down);
        let mut nearly = i32::MAX;
        if shown(row) {
            let mut adjacent = i32::MAX;
            for down in downs {
                let number = numbers[down as usize];
                if shown(down) {
                    adjacent = adjacent.min(number);
                } else {
                    nearly = nearly.min(number);
                }
            }
            if nearly == adjacent || nearly == i32::MAX {
                numbers[row as usize] = adjacent;
            } else {
                edges.push((row, nearly as u32));
                numbers[row as usize] = nearly;
            }
        } else {
            for down in downs {
                let number = if shown(down) {
                    down as i32
                } else {
                    numbers[down as usize]
                };
                nearly = nearly.min(number);
            }
            numbers[row as usize] = nearly;
        }
    }
    edges
}

pub fn dotted_edges(graph: &LinearGraph, kept: &BitVec) -> LinearGraph {
    let visible: Vec<bool> = graph.rows.iter().map(|commit| kept.get(*commit)).collect();
    let edges = dotted_rows(graph, &visible);
    let mut collapsed = CollapsedGraph::matching(graph, visible);
    for (up, down) in edges {
        collapsed.add_dotted(up, down);
    }
    collapsed.compile(graph)
}

// `VcsUserUtil.getNameFromEmail`.
fn name_from_email(email: &str) -> Option<&str> {
    email.find('@').filter(|at| *at > 0).map(|at| &email[..at])
}

// `VcsUserUtil.getNameInStandardForm`.
fn standard_form(name: &str) -> String {
    static NAME: OnceLock<Regex> = OnceLock::new();
    let pattern = NAME.get_or_init(|| {
        Regex::new(r"(?-u)^(\w+)[[:punct:]\s](\w+)$").expect("valid fixed expression")
    });
    if let Some(parts) = pattern.captures(name) {
        return format!(
            "{} {}",
            parts[1].to_ascii_lowercase(),
            parts[2].to_ascii_lowercase()
        );
    }
    if name.chars().all(|c| (' '..='~').contains(&c)) {
        name.to_ascii_lowercase()
    } else {
        name.to_string()
    }
}

// `VcsUserUtil.getUserName`.
fn user_name(person: &Person) -> &str {
    if person.name.is_empty() {
        name_from_email(&person.email).unwrap_or(&person.email)
    } else {
        &person.name
    }
}

pub(crate) fn same_person(a: &Person, b: &Person) -> bool {
    standard_form(user_name(a)) == standard_form(user_name(b))
}

// `VcsUserUtil.toExactString`.
fn exact_string(person: &Person) -> String {
    match (person.name.is_empty(), person.email.is_empty()) {
        (true, _) => person.email.to_string(),
        (_, true) => person.name.to_string(),
        _ => format!("{} <{}>", person.name, person.email),
    }
}

// `VcsLogUserResolverBase.resolveUserName`.
fn users_named<'a>(people: &'a [Person], name: &str) -> Vec<&'a Person> {
    let standard = standard_form(name);
    people
        .iter()
        .filter(|person| {
            (!person.name.is_empty() && standard_form(&person.name) == standard)
                || name_from_email(&person.email)
                    .is_some_and(|name| standard_form(name) == standard)
        })
        .collect()
}

// `VcsLogUserResolverBase.resolveCurrentUser`.
fn current_users<'a>(people: &'a [Person], me: &Person) -> Vec<&'a Person> {
    let mut users = users_named(people, &me.name);
    let Some(email_name) = name_from_email(&me.email) else {
        return users;
    };
    // ivan@ivanov.com and ivan@petrov.com share the same part and are different people.
    let emails: HashSet<String> = users.iter().map(|user| user.email.to_lowercase()).collect();
    for candidate in users_named(people, email_name) {
        if emails.contains(&candidate.email.to_lowercase()) && !users.contains(&candidate) {
            users.push(candidate);
        }
    }
    users
}

// What git must search for: `name <email>`, exact, and not the name chosen by the frontend.
fn resolve_users(values: &[String], people: &[Person], me: Option<&Person>) -> Vec<String> {
    let mut exact = Vec::new();
    for value in values {
        let users = match me {
            Some(me) if value == ME => current_users(people, me),
            None if value == ME => Vec::new(),
            _ => users_named(people, value),
        };
        for user in users {
            let text = exact_string(user);
            if !exact.contains(&text) {
                exact.push(text);
            }
        }
    }
    exact
}

fn hash_like(text: &str) -> bool {
    HASH_LENGTHS.contains(&text.len()) && text.chars().all(|c| c.is_ascii_hexdigit())
}

// `VcsLogFilterObject.fromHash`: a text that has only hashes.
fn hashes_of(filter: &LogFilter) -> Vec<&str> {
    let words: Vec<&str> = if filter.hashes.is_empty() {
        filter
            .text_filter()
            .map_or(Vec::new(), |text| text.text.split_whitespace().collect())
    } else {
        filter.hashes.iter().map(String::as_str).collect()
    };
    if words.iter().all(|word| hash_like(word)) {
        words
    } else {
        Vec::new()
    }
}

// `storage.findCommitId(CommitIdByStringCondition(...))`.
fn commit_with_prefix(store: &CommitStore, prefix: &str) -> Option<u32> {
    let prefix = prefix.to_ascii_lowercase();
    (0..store.len()).find(|commit| store.get(*commit).hash.starts_with(&prefix))
}

pub type RunGit<'a> = dyn FnMut(&[String]) -> Result<String, GitError> + 'a;

struct Lister<'a, 'b> {
    store: &'a CommitStore,
    index: HashMap<&'a str, u32>,
    git: &'a mut RunGit<'b>,
}

impl<'a, 'b> Lister<'a, 'b> {
    fn new(store: &'a CommitStore, git: &'a mut RunGit<'b>) -> Self {
        let commits = 0..store.len();
        Self {
            store,
            index: commits
                .map(|commit| (&*store.get(commit).hash, commit))
                .collect(),
            git,
        }
    }

    // Only the hashes: the view is built in memory. A commit that is not loaded is from after the
    // log was read.
    fn list(&mut self, filter: &LogFilter, kept: &mut BitVec) -> Result<(), GitError> {
        let mut command = Vec::new();
        if !filter.users.is_empty() {
            command.extend(["-c".to_string(), "log.mailmap=false".to_string()]);
        }
        command.extend(["--literal-pathspecs", "log", "--format=%H"].map(String::from));
        let has_head = self
            .store
            .refs()
            .all
            .iter()
            .any(|git_ref| git_ref.kind == RefKind::Head);
        let mut args = git_args(filter);
        if !has_head {
            // The revisions end at `--`: after it, `HEAD` would be a path.
            let end = args
                .iter()
                .position(|arg| arg == "--")
                .unwrap_or(args.len());
            if let Some(head) = args[..end].iter().position(|arg| arg == HEAD) {
                args.remove(head);
            }
        }
        command.extend(args);
        for hash in (self.git)(&command)?.lines() {
            if let Some(commit) = self.index.get(hash) {
                kept.set(*commit);
            }
        }
        Ok(())
    }

    // `VcsLogFiltererImpl.resolveCommit`, and if not, `GitLogProvider.resolveReference`.
    fn resolve(&mut self, name: &str) -> Result<Option<String>, GitError> {
        let store = self.store;
        let found = if name.len() == FULL_HASH_LENGTH && hash_like(name) {
            self.index.get(name).copied()
        } else {
            None
        };
        let found = found
            .or_else(|| {
                let refs = &store.refs().all;
                refs.iter()
                    .find(|git_ref| git_ref.kind != RefKind::Tag && &*git_ref.name == name)
                    .map(|git_ref| git_ref.commit)
            })
            .or_else(|| {
                (name.len() >= SHORT_HASH_LENGTH && hash_like(name))
                    .then(|| commit_with_prefix(store, name))
                    .flatten()
            });
        if let Some(commit) = found {
            return Ok(Some(store.get(commit).hash.to_string()));
        }
        let args = ["rev-parse", "--verify", "-q", "--end-of-options"]
            .map(String::from)
            .into_iter()
            .chain([format!("{name}^{{commit}}")])
            .collect::<Vec<_>>();
        match (self.git)(&args) {
            Ok(output) => Ok(Some(output.trim().to_string())),
            Err(GitError::Failed { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

// `applyHashFilter` with `vcs.log.filter.messages.by.hash`: the commits with those hashes and
// those that name them in their message, whether or not they match the other filters.
// `fromPatternsList` gives a regex with `Pattern.quote` that git does not understand; with an
// index, IntelliJ searches for any of the texts, and being hexadecimal this regex does the same.
fn select_hashes(
    store: &CommitStore,
    hashes: &[&str],
    git: &mut RunGit,
) -> Result<Option<BitVec>, GitError> {
    let mut kept = BitVec::new(store.len() as usize);
    for commit in hashes
        .iter()
        .filter_map(|hash| commit_with_prefix(store, hash))
    {
        kept.set(commit);
    }
    let messages = LogFilter {
        text: Some(TextFilter {
            text: hashes.join("|"),
            regex: hashes.len() > 1,
            match_case: false,
        }),
        ..LogFilter::default()
    };
    Lister::new(store, git).list(&messages, &mut kept)?;
    Ok((kept.count() > 0).then_some(kept))
}

// `VcsLogFiltererImpl.filter`, without stages (decision 8 of the plan): if git is needed, it is
// asked for everything.
pub fn select(
    store: &CommitStore,
    me: Option<&Person>,
    filter: &LogFilter,
    git: &mut RunGit,
) -> Result<Selection, GitError> {
    let nothing = || Selection::Commits(Arc::new(BitVec::new(store.len() as usize)));
    let hashes = hashes_of(filter);
    if !hashes.is_empty() && !store.is_empty() {
        if let Some(kept) = select_hashes(store, &hashes, git)? {
            return Ok(Selection::Commits(Arc::new(kept)));
        }
    }

    let branches = filter.branch_refs(store);
    let revisions: Vec<u32> = filter
        .revisions
        .iter()
        .filter(|hash| hash.len() == FULL_HASH_LENGTH)
        .filter_map(|hash| commit_with_prefix(store, hash))
        .collect();
    let mut heads: Vec<u32> = branches.iter().map(|git_ref| git_ref.commit).collect();
    heads.extend(&revisions);
    let by_heads = !filter.branches.is_empty() || !filter.revisions.is_empty();
    if !filter.has_details() && filter.ranges.is_empty() {
        return Ok(match (by_heads, heads.is_empty()) {
            (false, _) => Selection::All,
            (true, true) => nothing(),
            (true, false) => Selection::Heads(Arc::new(reachable(store, &heads))),
        });
    }
    if store.is_empty() {
        return Ok(nothing());
    }

    let mut resolved = filter.clone();
    // Only what exists reaches git: a made-up name can be nothing other than absent.
    resolved.branches = branches
        .iter()
        .map(|git_ref| git_ref.name.to_string())
        .collect();
    resolved.revisions = revisions
        .iter()
        .map(|commit| store.get(*commit).hash.to_string())
        .collect();
    if !filter.users.is_empty() {
        resolved.users = resolve_users(&filter.users, store.people().all(), me);
        if resolved.users.is_empty() {
            return Ok(nothing());
        }
    }
    let mut lister = Lister::new(store, git);
    let mut kept = BitVec::new(store.len() as usize);
    if filter.ranges.is_empty() {
        if by_heads && heads.is_empty() {
            return Ok(nothing());
        }
        lister.list(&resolved, &mut kept)?;
        if by_heads {
            kept.intersect(&reachable(store, &heads));
        }
        return Ok(Selection::Commits(Arc::new(kept)));
    }

    // `filteredDetailsInVcs`: if no range resolves there is nothing, not even from the branches.
    let mut ranges = Vec::new();
    for (exclusive, inclusive) in &filter.ranges {
        if let (Some(exclusive), Some(inclusive)) =
            (lister.resolve(exclusive)?, lister.resolve(inclusive)?)
        {
            ranges.push((exclusive, inclusive));
        }
    }
    if ranges.is_empty() {
        return Ok(nothing());
    }
    // The branches and each range are joined and need their order: with `git log branch a..b`,
    // what is below `a` would also disappear from the branch.
    resolved.ranges.clear();
    if !heads.is_empty() {
        lister.list(&resolved, &mut kept)?;
    }
    resolved.branches.clear();
    resolved.revisions.clear();
    for range in ranges {
        resolved.ranges = vec![range];
        lister.list(&resolved, &mut kept)?;
    }
    Ok(Selection::Commits(Arc::new(kept)))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Highlight {
    pub bold: bool,
    pub muted: bool,
    pub branch: bool,
}

// Which highlighters are active in a view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HighlightRules {
    pub my_commits: bool,
    pub current_branch: bool,
}

// `MyCommitsHighlighter`, `MergeCommitsHighlighter` and `CurrentBranchHighlighter`.
pub struct Highlighter {
    // Per person.
    mine: Vec<bool>,
    single_user: bool,
    // What is reachable from HEAD, and the name of the branch, or `HEAD` if there is none.
    current: Option<(BitVec, String)>,
}

impl Highlighter {
    pub fn new(store: &CommitStore, me: Option<&Person>) -> Self {
        let mine: Vec<bool> = store
            .people()
            .all()
            .iter()
            .map(|person| me.is_some_and(|me| same_person(me, person)))
            .collect();
        let refs = store.refs();
        let head = refs
            .all
            .iter()
            .find(|git_ref| git_ref.kind == RefKind::Head);
        Self {
            single_user: mine.iter().all(|mine| *mine),
            mine,
            current: head.map(|head| {
                let name = refs.current_name().unwrap_or(HEAD).to_string();
                (reachable(store, &[head.commit]), name)
            }),
        }
    }

    // `VcsLogUtil.getSingleFilteredBranch`.
    pub(crate) fn single_branch<'a>(store: &'a CommitStore, filter: &LogFilter) -> Option<&'a str> {
        match filter.branch_refs(store)[..] {
            [only] => Some(&*only.name),
            _ => None,
        }
    }

    pub fn rules(&self, store: &CommitStore, filter: &LogFilter) -> HighlightRules {
        let by_me = !filter.users.is_empty() && filter.users.iter().all(|user| user == ME);
        let single = Self::single_branch(store, filter);
        HighlightRules {
            my_commits: !by_me && !self.single_user,
            current_branch: self
                .current
                .as_ref()
                .is_some_and(|(_, name)| single != Some(HEAD) && single != Some(name.as_str())),
        }
    }

    pub fn of(&self, store: &CommitStore, commit: u32, rules: HighlightRules) -> Highlight {
        let data = store.get(commit);
        Highlight {
            bold: rules.my_commits && self.mine[data.author as usize],
            muted: data.parents.len() >= 2,
            branch: rules.current_branch
                && self
                    .current
                    .as_ref()
                    .is_some_and(|(contained, _)| contained.get(commit)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::collapse::{self, GraphTarget};
    use crate::log::graph::EdgeStyle;
    use crate::log::store;
    use crate::log::view::{Permanent, SortType, View, ViewPatch, ViewSpec};
    use crate::test_support::TempRepo;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn text(value: &str) -> Option<TextFilter> {
        Some(TextFilter {
            text: value.to_string(),
            regex: false,
            match_case: false,
        })
    }

    fn regex(value: &str) -> Option<TextFilter> {
        Some(TextFilter {
            text: value.to_string(),
            regex: true,
            match_case: false,
        })
    }

    fn has(args: &[String], expected: &str) -> bool {
        args.iter().any(|arg| arg == expected)
    }

    #[test]
    fn default_args_include_head_branches_remotes_tags() {
        let args = git_args(&LogFilter::default());

        let start = args.iter().position(|arg| arg == "HEAD").unwrap();
        assert_eq!(
            args[start..],
            ["HEAD", "--branches", "--remotes", "--tags", "--"]
        );
        assert!(!has(&args, "--end-of-options"));
    }

    #[test]
    fn fixed_text_args() {
        let args = git_args(&LogFilter {
            text: text("fix"),
            ..LogFilter::default()
        });

        assert!(has(&args, "--grep=fix"));
        assert!(has(&args, "--fixed-strings"));
        assert!(has(&args, "--regexp-ignore-case"));
        assert!(!has(&args, "--extended-regexp"));
    }

    #[test]
    fn regex_args_only_with_a_valid_metacharacter() {
        let is_regex = |value: &str| {
            let args = git_args(&LogFilter {
                text: regex(value),
                ..LogFilter::default()
            });
            assert!(has(&args, &format!("--grep={value}")));
            assert_ne!(
                has(&args, "--extended-regexp"),
                has(&args, "--fixed-strings")
            );
            has(&args, "--extended-regexp")
        };

        assert!(!is_regex("foo"));
        assert!(is_regex("fo+"));
        assert!(!is_regex("fo("));
    }

    #[test]
    fn metacharacter_without_regex_enabled_is_text() {
        let args = git_args(&LogFilter {
            text: text("fo+"),
            ..LogFilter::default()
        });

        assert!(has(&args, "--fixed-strings"));
    }

    #[test]
    fn match_case_removes_ignore_case() {
        let args = git_args(&LogFilter {
            text: Some(TextFilter {
                text: "Foo".to_string(),
                regex: false,
                match_case: true,
            }),
            ..LogFilter::default()
        });

        assert!(!has(&args, "--regexp-ignore-case"));
    }

    #[test]
    fn blank_text_does_not_filter() {
        let args = git_args(&LogFilter {
            text: text("   "),
            ..LogFilter::default()
        });

        assert!(!args.iter().any(|arg| arg.starts_with("--grep")));
        assert!(has(&args, "--extended-regexp"));
    }

    #[test]
    fn author_args_escaped_and_anchored() {
        let args = git_args(&LogFilter {
            users: strings(&["Alice (x)"]),
            ..LogFilter::default()
        });
        assert!(has(&args, r"--author=^Alice \(x\)$"), "{args:?}");

        let args = git_args(&LogFilter {
            users: strings(&["Alice (x)", r"Bob\ <l-m@a.b>"]),
            ..LogFilter::default()
        });
        assert!(
            has(&args, r"--author=^Alice \(x\)$|^Bob\\ <l\-m@a\.b>$"),
            "{args:?}"
        );
    }

    #[test]
    fn with_fixed_text_the_author_is_not_a_regex_either() {
        let args = git_args(&LogFilter {
            text: text("fix"),
            users: strings(&["Alice (x)", r"Bob\"]),
            ..LogFilter::default()
        });

        assert!(has(&args, "--author=Alice (x)"));
        assert!(has(&args, r"--author=Bob\\"));
    }

    #[test]
    fn path_args_at_the_end() {
        let args = git_args(&LogFilter {
            paths: strings(&["src/a b.txt", "-odd"]),
            ..LogFilter::default()
        });

        let start = args.iter().position(|arg| arg == "--full-history").unwrap();
        assert_eq!(args[start + 1], "--simplify-merges");
        assert_eq!(args[args.len() - 3..], ["--", "src/a b.txt", "-odd"]);
    }

    #[test]
    fn date_args() {
        let args = git_args(&LogFilter {
            since: Some(1_700_000_000),
            until: Some(1_800_000_000),
            ..LogFilter::default()
        });

        assert!(has(&args, "--after=1700000000"));
        assert!(has(&args, "--before=1800000000"));
    }

    #[test]
    fn branch_args_after_the_end_of_options() {
        let args = git_args(&LogFilter {
            branches: strings(&["-x", "main"]),
            ..LogFilter::default()
        });

        assert_eq!(
            args[args.len() - 4..],
            ["--end-of-options", "-x", "main", "--"]
        );
        assert!(!has(&args, "--branches"));
    }

    #[test]
    fn range_args() {
        let args = git_args(&LogFilter {
            ranges: vec![("a1".to_string(), "b2".to_string())],
            ..LogFilter::default()
        });

        assert_eq!(args[args.len() - 3..], ["--end-of-options", "a1..b2", "--"]);
    }

    // A real `git log`, without going through the tasks.
    fn real_git(repo: &TempRepo) -> impl FnMut(&[String]) -> Result<String, GitError> + '_ {
        |args| {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            crate::git::run(repo.path(), &args)
                .map(|output| String::from_utf8_lossy(&output).into_owned())
        }
    }

    fn no_git(_: &[String]) -> Result<String, GitError> {
        panic!("git was not supposed to be launched");
    }

    fn subjects_of(store: &CommitStore, view: &View) -> Vec<String> {
        view.graph
            .rows
            .iter()
            .map(|commit| store.get(*commit).subject.to_string())
            .collect()
    }

    fn filtered_view(repo: &TempRepo, sort: SortType, filter: LogFilter) -> (CommitStore, View) {
        let store = store::load(repo.path()).unwrap();
        let selection = select(&store, None, &filter, &mut real_git(repo)).unwrap();
        let view = view_of(&store, sort, filter, selection);
        (store, view)
    }

    fn view_of(
        store: &CommitStore,
        sort: SortType,
        filter: LogFilter,
        selection: Selection,
    ) -> View {
        let permanent = Arc::new(Permanent::from_store(store, store.refs()));
        let spec = ViewSpec {
            sort,
            filter,
            ..ViewSpec::default()
        };
        View::filtered(permanent, &spec, selection)
    }

    // main: base - one - two, and `other`, which branches off base with one commit.
    fn repo_with_branch() -> TempRepo {
        let repo = TempRepo::new();
        repo.commit_dated("a.txt", "base", "2026-01-01T12:00:00");
        repo.commit_dated("b.txt", "one fix", "2026-01-02T12:00:00");
        repo.git(&["checkout", "-q", "-b", "other", "HEAD~1"]);
        repo.commit_dated("c.txt", "side fix", "2026-01-03T12:00:00");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit_dated("d.txt", "two", "2026-01-04T12:00:00");
        repo
    }

    // Commits of the branch and of main interleaved by date, with the merge on top.
    fn repo_interleaved() -> TempRepo {
        let repo = TempRepo::new();
        let commit = |subject: &str, date: u32| {
            let date = format!("2026-01-0{date}T12:00:00");
            repo.commit_dated(
                &format!("{}.txt", subject.replace(' ', "-")),
                subject,
                &date,
            );
        };
        commit("base", 1);
        repo.git(&["checkout", "-q", "-b", "other"]);
        commit("branch 1 keep", 2);
        repo.git(&["checkout", "-q", "main"]);
        commit("main 1 keep", 3);
        repo.git(&["checkout", "-q", "other"]);
        commit("branch 2", 4);
        repo.git(&["checkout", "-q", "main"]);
        commit("main 2", 5);
        repo.merge_dated("other", "merge keep", "2026-01-06T12:00:00");
        repo.git(&["branch", "-D", "other"]);
        repo
    }

    #[test]
    fn hex_text_is_tried_first_as_a_hash() {
        let repo = repo_with_branch();
        let hash = repo.git(&["rev-parse", "main~1"]);
        let filter = LogFilter {
            text: text(&hash.trim()[..9]),
            ..LogFilter::default()
        };

        let store = store::load(repo.path()).unwrap();
        let selection = select(&store, None, &filter, &mut real_git(&repo)).unwrap();

        let view = view_of(&store, SortType::Off, filter, selection);
        assert_eq!(subjects_of(&store, &view), ["one fix"]);
    }

    #[test]
    fn hash_that_does_not_exist_stays_as_text() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "base");
        repo.commit("a.txt", b"2", "fixes abcdef12 from yesterday");
        let (store, view) = filtered_view(
            &repo,
            SortType::Off,
            LogFilter {
                text: text("abcdef12"),
                ..LogFilter::default()
            },
        );

        assert_eq!(
            subjects_of(&store, &view),
            ["fixes abcdef12 from yesterday"]
        );
    }

    #[test]
    fn hash_filter_rules_over_the_others() {
        let repo = repo_with_branch();
        let hash = repo.git(&["rev-parse", "other"]);
        let (store, view) = filtered_view(
            &repo,
            SortType::Off,
            LogFilter {
                hashes: vec![hash.trim().to_uppercase()],
                users: strings(&["Nobody"]),
                ..LogFilter::default()
            },
        );

        assert_eq!(subjects_of(&store, &view), ["side fix"]);
    }

    #[test]
    fn hash_that_does_not_look_like_one_matches_nothing() {
        let repo = repo_with_branch();
        let filter = LogFilter {
            hashes: strings(&[""]),
            ..LogFilter::default()
        };
        let store = store::load(repo.path()).unwrap();

        let selection = select(&store, None, &filter, &mut no_git).unwrap();

        assert_eq!(
            view_of(&store, SortType::Off, filter, selection)
                .graph
                .len(),
            4
        );
    }

    #[test]
    fn hash_also_searches_in_messages() {
        let repo = TempRepo::new();
        let hash = repo.commit("a.txt", b"1", "base");
        repo.commit("a.txt", b"2", "another");
        repo.commit(
            "a.txt",
            b"3",
            &format!("Revert {}", hash[..9].to_uppercase()),
        );
        let (store, view) = filtered_view(
            &repo,
            SortType::Off,
            LogFilter {
                text: text(&hash[..9]),
                ..LogFilter::default()
            },
        );

        assert_eq!(
            subjects_of(&store, &view),
            [
                format!("Revert {}", hash[..9].to_uppercase()),
                "base".to_string()
            ]
        );
    }

    #[test]
    fn hash_ignores_the_other_filters_in_messages_too() {
        let repo = TempRepo::new();
        let hash = repo.commit("a.txt", b"1", "base");
        repo.commit("a.txt", b"2", &format!("Revert {}", &hash[..9]));
        let filter = LogFilter {
            hashes: vec![hash[..9].to_string()],
            branches: strings(&["does-not-exist"]),
            paths: strings(&["no.txt"]),
            ..LogFilter::default()
        };
        let mut seen = Vec::new();
        let store = store::load(repo.path()).unwrap();
        let mut git = real_git(&repo);

        select(&store, None, &filter, &mut |args| {
            seen = args.to_vec();
            git(args)
        })
        .unwrap();

        assert!(has(&seen, "HEAD"), "{seen:?}");
        assert!(has(&seen, "--fixed-strings"), "{seen:?}");
        assert!(!has(&seen, "no.txt"), "{seen:?}");
    }

    #[test]
    fn several_hashes_search_for_any_in_messages() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "base");
        repo.commit("a.txt", b"2", "names aaaaaaa1");
        repo.commit("a.txt", b"3", "names bbbbbbb2");
        repo.commit("a.txt", b"4", "names nothing");
        let (store, view) = filtered_view(
            &repo,
            SortType::Off,
            LogFilter {
                text: text("aaaaaaa1 bbbbbbb2"),
                ..LogFilter::default()
            },
        );

        assert_eq!(
            subjects_of(&store, &view),
            ["names bbbbbbb2", "names aaaaaaa1"]
        );
    }

    #[test]
    fn hash_without_result_falls_back_to_the_other_filters() {
        let repo = repo_with_branch();
        let (store, view) = filtered_view(
            &repo,
            SortType::Off,
            LogFilter {
                hashes: strings(&["abcdef12"]),
                branches: strings(&["other"]),
                ..LogFilter::default()
            },
        );

        assert_eq!(subjects_of(&store, &view), ["side fix", "base"]);
    }

    fn by_branches(repo: &TempRepo, patterns: &[&str]) -> Vec<String> {
        let store = store::load(repo.path()).unwrap();
        let filter = LogFilter {
            branches: strings(patterns),
            ..LogFilter::default()
        };
        let selection = select(&store, None, &filter, &mut no_git).unwrap();
        subjects_of(&store, &view_of(&store, SortType::Off, filter, selection))
    }

    #[test]
    fn branch_by_regex_matches_the_whole_name() {
        let repo = repo_with_branch();

        assert_eq!(by_branches(&repo, &["ot.*"]), ["side fix", "base"]);
        assert!(by_branches(&repo, &["ot.?"]).is_empty());
    }

    #[test]
    fn branch_excluded_with_dash() {
        let repo = repo_with_branch();
        repo.git(&["checkout", "-q", "other"]);

        // HEAD is on `other`: only `main` remains.
        assert_eq!(
            by_branches(&repo, &["-other", "-HEAD"]),
            ["two", "one fix", "base"]
        );
    }

    #[test]
    fn only_exclusions_includes_the_rest() {
        let repo = repo_with_branch();
        repo.git(&["branch", "-D", "other"]);
        repo.git(&["branch", "new", "main~1"]);

        assert_eq!(by_branches(&repo, &["-new"]), ["two", "one fix", "base"]);
        assert!(by_branches(&repo, &["-.*"]).is_empty());
    }

    #[test]
    fn inclusion_and_exclusion_by_regex() {
        let repo = repo_with_branch();
        repo.git(&["branch", "ma-old", "other"]);

        assert_eq!(
            by_branches(&repo, &["ma.*", "-ma-.*"]),
            ["two", "one fix", "base"]
        );
    }

    #[test]
    fn exact_name_wins_over_dash_and_regex() {
        let repo = repo_with_branch();
        repo.git(&["update-ref", "refs/heads/-x", "other"]);
        repo.git(&["update-ref", "refs/heads/m.in", "other"]);

        assert_eq!(by_branches(&repo, &["-x"]), ["side fix", "base"]);
        assert_eq!(by_branches(&repo, &["m.in"]), ["side fix", "base"]);
    }

    #[test]
    fn invalid_regex_is_a_name_that_does_not_exist() {
        let repo = repo_with_branch();

        assert!(by_branches(&repo, &["ot("]).is_empty());
    }

    #[test]
    fn only_resolved_names_reach_git() {
        let repo = repo_with_branch();
        let filter = LogFilter {
            branches: strings(&["ot.*"]),
            text: text("fix"),
            ..LogFilter::default()
        };
        let mut seen = Vec::new();
        let store = store::load(repo.path()).unwrap();
        let mut git = real_git(&repo);

        select(&store, None, &filter, &mut |args| {
            seen = args.to_vec();
            git(args)
        })
        .unwrap();

        assert_eq!(seen[seen.len() - 3..], ["--end-of-options", "other", "--"]);
    }

    #[test]
    fn revision_shows_what_is_reachable_from_it() {
        let repo = repo_with_branch();
        let hash = repo.git(&["rev-parse", "main~1"]);
        let store = store::load(repo.path()).unwrap();
        let filter = LogFilter {
            revisions: vec![hash.trim().to_string()],
            ..LogFilter::default()
        };

        let selection = select(&store, None, &filter, &mut no_git).unwrap();

        let view = view_of(&store, SortType::Off, filter, selection);
        assert_eq!(subjects_of(&store, &view), ["one fix", "base"]);
        assert!(view.collapsible());
    }

    #[test]
    fn revision_and_branch_add_up() {
        let repo = repo_with_branch();
        let hash = repo.git(&["rev-parse", "main~1"]);
        let store = store::load(repo.path()).unwrap();
        let filter = LogFilter {
            revisions: vec![hash.trim().to_string()],
            branches: strings(&["other"]),
            ..LogFilter::default()
        };

        let selection = select(&store, None, &filter, &mut no_git).unwrap();

        let view = view_of(&store, SortType::Off, filter, selection);
        assert_eq!(view.graph.len(), 3);
    }

    #[test]
    fn revision_with_text_reaches_git() {
        let repo = repo_with_branch();
        let hash = repo.git(&["rev-parse", "main~1"]);
        let (store, view) = filtered_view(
            &repo,
            SortType::Off,
            LogFilter {
                revisions: vec![hash.trim().to_string()],
                text: text("fix"),
                ..LogFilter::default()
            },
        );

        assert_eq!(subjects_of(&store, &view), ["one fix"]);
    }

    #[test]
    fn revision_that_does_not_exist_gives_empty_view() {
        let repo = repo_with_branch();
        let store = store::load(repo.path()).unwrap();
        let filter = LogFilter {
            revisions: vec!["a".repeat(40)],
            text: text("fix"),
            ..LogFilter::default()
        };

        let selection = select(&store, None, &filter, &mut no_git).unwrap();

        assert!(view_of(&store, SortType::Off, filter, selection)
            .graph
            .is_empty());
    }

    #[test]
    fn unresolved_range_with_branch_gives_empty_view() {
        let repo = repo_with_branch();
        let (_, view) = filtered_view(
            &repo,
            SortType::Off,
            LogFilter {
                ranges: vec![("does-not-exist".to_string(), "main".to_string())],
                branches: strings(&["other"]),
                ..LogFilter::default()
            },
        );

        assert!(view.graph.is_empty());
    }

    #[test]
    fn several_ranges_are_joined_and_an_unresolved_one_does_not_count() {
        let repo = repo_with_branch();
        let (store, view) = filtered_view(
            &repo,
            SortType::Off,
            LogFilter {
                ranges: vec![
                    ("main~1".to_string(), "main".to_string()),
                    ("does-not-exist".to_string(), "main".to_string()),
                    ("main~2".to_string(), "other".to_string()),
                ],
                ..LogFilter::default()
            },
        );

        assert_eq!(subjects_of(&store, &view), ["two", "side fix"]);
    }

    #[test]
    fn path_named_head_without_head() {
        let repo = TempRepo::new();
        repo.commit("HEAD", b"1", "touches head");
        repo.commit("a.txt", b"1", "another");
        repo.git(&["checkout", "-q", "--orphan", "new"]);
        let (store, view) = filtered_view(
            &repo,
            SortType::Off,
            LogFilter {
                paths: strings(&["HEAD"]),
                ..LogFilter::default()
            },
        );

        assert_eq!(subjects_of(&store, &view), ["touches head"]);
    }

    #[test]
    fn current_branch_with_pattern_filter_counts_as_that_branch() {
        let repo = repo_with_branch();
        let filter = LogFilter {
            branches: strings(&["ma.*"]),
            ..LogFilter::default()
        };

        assert!(highlights(&repo, &filter).iter().all(|found| !found.branch));
    }

    #[test]
    fn branch_filter_with_text_passes_the_branch_to_git() {
        let repo = repo_with_branch();
        let filter = LogFilter {
            branches: strings(&["other"]),
            text: text("fix"),
            ..LogFilter::default()
        };
        let mut seen = Vec::new();
        let store = store::load(repo.path()).unwrap();
        let mut git = real_git(&repo);

        let selection = select(&store, None, &filter, &mut |args| {
            seen = args.to_vec();
            git(args)
        })
        .unwrap();

        assert_eq!(
            seen[seen.len() - 3..],
            ["--end-of-options", "other", "--"],
            "{seen:?}"
        );
        let view = view_of(&store, SortType::Off, filter, selection);
        assert_eq!(subjects_of(&store, &view), ["side fix"]);
    }

    #[test]
    fn branch_that_does_not_exist_does_not_reach_git() {
        let repo = repo_with_branch();
        let store = store::load(repo.path()).unwrap();
        let filter = LogFilter {
            branches: strings(&["does-not-exist"]),
            text: text("fix"),
            ..LogFilter::default()
        };

        let selection = select(&store, None, &filter, &mut no_git).unwrap();

        let view = view_of(&store, SortType::Off, filter, selection);
        assert!(view.graph.is_empty());
    }

    #[test]
    fn a_pattern_with_dashes_is_an_exclusion_and_not_an_option() {
        let repo = repo_with_branch();
        let filter = LogFilter {
            branches: strings(&["--all"]),
            text: text("fix"),
            ..LogFilter::default()
        };
        let mut seen = Vec::new();
        let store = store::load(repo.path()).unwrap();
        let mut git = real_git(&repo);

        select(&store, None, &filter, &mut |args| {
            seen = args.to_vec();
            git(args)
        })
        .unwrap();

        assert!(!has(&seen, "--all"), "{seen:?}");
    }

    #[test]
    fn branch_filter_only_leaves_what_is_reachable() {
        let repo = repo_with_branch();
        let store = store::load(repo.path()).unwrap();
        let filter = LogFilter {
            branches: strings(&["other"]),
            ..LogFilter::default()
        };

        let selection = select(&store, None, &filter, &mut no_git).unwrap();

        let view = view_of(&store, SortType::Off, filter, selection);
        assert_eq!(subjects_of(&store, &view), ["side fix", "base"]);
        // Without a details filter, the edges are the real ones.
        assert_eq!(view.graph.downs.of(0)[0].style, EdgeStyle::Usual);
    }

    #[test]
    fn head_counts_as_branch() {
        let repo = repo_with_branch();
        let store = store::load(repo.path()).unwrap();
        let filter = LogFilter {
            branches: strings(&["HEAD"]),
            ..LogFilter::default()
        };

        let selection = select(&store, None, &filter, &mut no_git).unwrap();

        let view = view_of(&store, SortType::Off, filter, selection);
        assert_eq!(subjects_of(&store, &view), ["two", "one fix", "base"]);
    }

    #[test]
    fn a_tag_is_not_a_branch_filter() {
        let repo = repo_with_branch();
        repo.git(&["tag", "v1", "other"]);
        let store = store::load(repo.path()).unwrap();
        let filter = LogFilter {
            branches: strings(&["v1"]),
            ..LogFilter::default()
        };

        let selection = select(&store, None, &filter, &mut no_git).unwrap();

        assert!(view_of(&store, SortType::Off, filter, selection)
            .graph
            .is_empty());
    }

    #[test]
    fn filtering_joins_what_remains_with_dotted_edges() {
        let repo = repo_with_branch();
        let (store, view) = filtered_view(
            &repo,
            SortType::Off,
            LogFilter {
                users: Vec::new(),
                paths: strings(&["d.txt", "a.txt"]),
                ..LogFilter::default()
            },
        );

        assert_eq!(subjects_of(&store, &view), ["two", "base"]);
        let edge = view.graph.downs.of(0)[0];
        assert_eq!((edge.down, edge.style), (1, EdgeStyle::Dotted));
    }

    #[test]
    fn filter_without_results_gives_empty_view() {
        let repo = repo_with_branch();
        let (_store, view) = filtered_view(
            &repo,
            SortType::Standard,
            LogFilter {
                text: text("appears nowhere"),
                ..LogFilter::default()
            },
        );

        assert!(view.graph.is_empty());
        assert!(crate::log::print::elements_in_rows(&view, 0, 0).is_empty());
    }

    #[test]
    fn date_filter_with_git() {
        let repo = repo_with_branch();
        let store = store::load(repo.path()).unwrap();
        let time = |index: u32| store.get(index).commit_time;
        let (_, view) = filtered_view(
            &repo,
            SortType::Off,
            LogFilter {
                since: Some(time(1)),
                until: Some(time(1)),
                ..LogFilter::default()
            },
        );

        assert_eq!(view.graph.rows, [1]);
    }

    #[test]
    fn branch_name_with_dash_is_not_an_option() {
        let repo = repo_with_branch();
        repo.git(&["update-ref", "refs/heads/-x", "other"]);
        repo.git(&["update-ref", "refs/heads/--all", "other"]);
        let store = store::load(repo.path()).unwrap();

        for name in ["-x", "--all"] {
            let filter = LogFilter {
                branches: strings(&[name]),
                text: text("fix"),
                ..LogFilter::default()
            };
            let selection = select(&store, None, &filter, &mut real_git(&repo)).unwrap();
            let view = view_of(&store, SortType::Off, filter, selection);
            assert_eq!(subjects_of(&store, &view), ["side fix"], "{name}");
        }
    }

    #[test]
    fn path_with_dash_or_magic_is_a_path() {
        let repo = TempRepo::new();
        repo.commit("-x", b"1", "with dash");
        repo.commit(":(top)a", b"1", "with magic");
        repo.commit("other.txt", b"1", "another");
        for (path, subject) in [("-x", "with dash"), (":(top)a", "with magic")] {
            let (store, view) = filtered_view(
                &repo,
                SortType::Off,
                LogFilter {
                    paths: strings(&[path]),
                    ..LogFilter::default()
                },
            );
            assert_eq!(subjects_of(&store, &view), [subject]);
        }
    }

    #[test]
    fn user_filter_resolves_to_the_exact_author() {
        let repo = repo_with_branch();
        repo.git(&[
            "-c",
            "user.name=Alice (x)",
            "-c",
            "user.email=a-b@c.d",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "by alice",
        ]);
        repo.git(&[
            "-c",
            "user.name=Alice (x) two",
            "-c",
            "user.email=e@c.d",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "by other",
        ]);
        let (store, view) = filtered_view(
            &repo,
            SortType::Off,
            LogFilter {
                users: strings(&["alice (X)"]),
                ..LogFilter::default()
            },
        );

        assert_eq!(subjects_of(&store, &view), ["by alice"]);
    }

    #[test]
    fn unknown_user_does_not_reach_git() {
        let repo = repo_with_branch();
        let store = store::load(repo.path()).unwrap();
        let filter = LogFilter {
            users: strings(&["Nobody"]),
            ..LogFilter::default()
        };

        let selection = select(&store, None, &filter, &mut no_git).unwrap();

        assert!(view_of(&store, SortType::Off, filter, selection)
            .graph
            .is_empty());
    }

    #[test]
    fn me_filter_uses_the_git_user() {
        let repo = repo_with_branch();
        repo.git(&[
            "-c",
            "user.name=Another",
            "-c",
            "user.email=another@example.com",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "foreign",
        ]);
        let store = store::load(repo.path()).unwrap();
        let me = Person {
            name: "Test".into(),
            email: "test@example.com".into(),
        };
        let filter = LogFilter {
            users: strings(&[ME]),
            ..LogFilter::default()
        };

        let selection = select(&store, Some(&me), &filter, &mut real_git(&repo)).unwrap();

        let view = view_of(&store, SortType::Off, filter, selection);
        assert_eq!(view.graph.len(), 4);
        assert!(!subjects_of(&store, &view).contains(&"foreign".to_string()));
    }

    #[test]
    fn range_with_references_and_hashes() {
        let repo = repo_with_branch();
        let base = repo.git(&["rev-parse", "main~2"]);
        for start in [base.trim(), "main~2"] {
            let (store, view) = filtered_view(
                &repo,
                SortType::Off,
                LogFilter {
                    ranges: vec![(start.to_string(), "main".to_string())],
                    ..LogFilter::default()
                },
            );
            assert_eq!(subjects_of(&store, &view), ["two", "one fix"]);
        }
    }

    #[test]
    fn range_that_does_not_resolve_gives_nothing() {
        let repo = repo_with_branch();
        let (_, view) = filtered_view(
            &repo,
            SortType::Off,
            LogFilter {
                ranges: vec![("--all".to_string(), "main".to_string())],
                ..LogFilter::default()
            },
        );

        assert!(view.graph.is_empty());
    }

    #[test]
    fn range_and_branch_are_joined() {
        let repo = repo_with_branch();
        let (store, view) = filtered_view(
            &repo,
            SortType::Off,
            LogFilter {
                ranges: vec![("main~1".to_string(), "main".to_string())],
                branches: strings(&["other"]),
                ..LogFilter::default()
            },
        );

        assert_eq!(subjects_of(&store, &view), ["two", "side fix", "base"]);
    }

    #[test]
    fn filtering_keeps_standard_order() {
        let repo = repo_interleaved();
        let (store, view) = filtered_view(
            &repo,
            SortType::Standard,
            LogFilter {
                text: text("keep"),
                ..LogFilter::default()
            },
        );

        assert_eq!(
            subjects_of(&store, &view),
            ["merge keep", "branch 1 keep", "main 1 keep"]
        );
    }

    #[test]
    fn filtering_keeps_linear_order() {
        let repo = repo_interleaved();
        let (store, view) = filtered_view(
            &repo,
            SortType::Linear,
            LogFilter {
                text: text("keep"),
                ..LogFilter::default()
            },
        );

        assert_eq!(view.spec.sort, SortType::Linear);
        assert_eq!(
            subjects_of(&store, &view),
            ["merge keep", "branch 1 keep", "main 1 keep"]
        );
    }

    #[test]
    fn changing_order_keeps_the_filter() {
        let repo = repo_interleaved();
        let (store, mut view) = filtered_view(
            &repo,
            SortType::Off,
            LogFilter {
                text: text("keep"),
                ..LogFilter::default()
            },
        );
        assert_eq!(
            subjects_of(&store, &view),
            ["merge keep", "main 1 keep", "branch 1 keep"]
        );

        view.apply(&ViewPatch {
            sort: Some(SortType::Standard),
            ..ViewPatch::default()
        });

        assert_eq!(
            subjects_of(&store, &view),
            ["merge keep", "branch 1 keep", "main 1 keep"]
        );
        assert_eq!(view.spec.filter.text, text("keep"));
    }

    #[test]
    fn no_collapse_with_details_filter() {
        let repo = repo_with_branch();
        let (_, mut view) = filtered_view(
            &repo,
            SortType::Off,
            LogFilter {
                paths: strings(&["d.txt", "a.txt"]),
                ..LogFilter::default()
            },
        );
        let before = view.graph.dump();
        assert!(!view.collapsible());

        collapse::collapse_all(&mut view);
        assert_eq!(view.graph.dump(), before);
        assert!(!view.spec.collapsed);

        assert!(!collapse::toggle(&mut view, GraphTarget::Node { row: 0 }));
        collapse::expand_all(&mut view);
        assert_eq!(view.graph.dump(), before);
    }

    #[test]
    fn no_collapse_with_details_filter_in_linear() {
        let repo = repo_interleaved();
        let (_, mut view) = filtered_view(
            &repo,
            SortType::Linear,
            LogFilter {
                text: text("keep"),
                ..LogFilter::default()
            },
        );
        let before = view.graph.dump();

        assert!(!collapse::toggle(&mut view, GraphTarget::Node { row: 0 }));
        collapse::expand_all(&mut view);

        assert_eq!(view.graph.dump(), before);
    }

    #[test]
    fn filter_requested_collapsed_does_not_stay_collapsed() {
        let repo = repo_with_branch();
        let store = store::load(repo.path()).unwrap();
        let permanent = Arc::new(Permanent::from_store(&store, store.refs()));
        let spec = ViewSpec {
            collapsed: true,
            ..ViewSpec::default()
        };
        let bits = Arc::new(reachable(&store, &[1]));

        let view = View::filtered(permanent, &spec, Selection::Commits(bits));

        assert!(!view.spec.collapsed);
    }

    #[test]
    fn with_branch_filter_it_does_collapse() {
        // The side branch is hidden; the main chain does collapse up to the branch.
        let repo = TempRepo::new();
        repo.commit("a.txt", b"0", "c0");
        for index in 1..6 {
            repo.commit("a.txt", index.to_string().as_bytes(), &format!("c{index}"));
        }
        repo.git(&["checkout", "-q", "-b", "outside", "HEAD~4"]);
        repo.commit("b.txt", b"1", "outside");
        repo.git(&["checkout", "-q", "main"]);
        let store = store::load(repo.path()).unwrap();
        let filter = LogFilter {
            branches: strings(&["main"]),
            ..LogFilter::default()
        };
        let selection = select(&store, None, &filter, &mut no_git).unwrap();
        let mut view = view_of(&store, SortType::Off, filter, selection);
        assert_eq!(view.graph.len(), 6);
        assert!(view.collapsible());

        collapse::collapse_all(&mut view);
        assert!(view.spec.collapsed);
        assert!(view.graph.len() < 6);
        assert!(view
            .graph
            .rows
            .iter()
            .all(|commit| { &*store.get(*commit).subject != "outside" }));

        collapse::expand_all(&mut view);
        // It goes back to what the filter lets through, not to all the commits.
        assert_eq!(view.graph.len(), 6);
        assert!(!view.spec.collapsed);
    }

    #[test]
    fn with_branch_filter_click_collapses_and_expands() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"0", "c0");
        for index in 1..6 {
            repo.commit("a.txt", index.to_string().as_bytes(), &format!("c{index}"));
        }
        repo.git(&["branch", "down", "HEAD~4"]);
        repo.git(&["checkout", "-q", "-b", "outside"]);
        repo.commit("b.txt", b"1", "outside");
        repo.git(&["checkout", "-q", "main"]);
        let store = store::load(repo.path()).unwrap();
        let filter = LogFilter {
            branches: strings(&["main"]),
            ..LogFilter::default()
        };
        let selection = select(&store, None, &filter, &mut no_git).unwrap();
        let mut view = view_of(&store, SortType::Off, filter, selection);
        let before = view.graph.dump();

        assert!(collapse::toggle(&mut view, GraphTarget::Node { row: 2 }));
        assert!(view.graph.len() < 6);
        let dotted = (0..view.graph.len())
            .find(|row| {
                view.graph
                    .downs
                    .of(*row)
                    .iter()
                    .any(|edge| edge.style == EdgeStyle::Dotted)
            })
            .unwrap();

        assert!(collapse::toggle(
            &mut view,
            GraphTarget::Node { row: dotted }
        ));
        assert_eq!(view.graph.dump(), before);
    }

    #[test]
    fn linear_with_branch_filter_hides_and_allows_click() {
        let repo = repo_interleaved();
        repo.git(&["checkout", "-q", "-b", "outside", "main~1"]);
        repo.commit("z.txt", b"1", "outside");
        repo.git(&["checkout", "-q", "main"]);
        let store = store::load(repo.path()).unwrap();
        let filter = LogFilter {
            branches: strings(&["main"]),
            ..LogFilter::default()
        };
        let selection = select(&store, None, &filter, &mut no_git).unwrap();
        let mut view = view_of(&store, SortType::Linear, filter, selection);

        assert_eq!(view.graph.len(), 6);
        assert!(!subjects_of(&store, &view).contains(&"outside".to_string()));
        assert_eq!(view.graph.downs.of(0).len(), 1);

        // "branch 1" carries the dotted edge. There is a hidden row above it: if the click did not
        // translate the rows, it would land on another commit.
        assert!(collapse::toggle(&mut view, GraphTarget::Node { row: 2 }));
        assert_eq!(view.graph.downs.of(0).len(), 2);
        assert_eq!(view.graph.len(), 6);

        collapse::collapse_all(&mut view);
        assert_eq!(view.graph.downs.of(0).len(), 1);
        collapse::expand_all(&mut view);
        assert_eq!(view.graph.downs.of(0).len(), 2);
        assert!(!subjects_of(&store, &view).contains(&"outside".to_string()));
    }

    #[test]
    fn reachable_goes_down_through_all_parents() {
        let repo = repo_interleaved();
        let store = store::load(repo.path()).unwrap();

        let all = reachable(&store, &[0]);
        assert_eq!(all.count(), store.len());

        let one = reachable(&store, &[store.len() - 1]);
        assert_eq!(one.count(), 1);
        assert!(one.get(store.len() - 1));
    }

    // `DottedFilterEdgesGeneratorTest`, with rows starting at 0.
    fn dotted(graph: &str, hidden: &[u32]) -> String {
        let graph = LinearGraph::parse(graph);
        let mut kept = BitVec::new(graph.rows.len());
        for row in (0..graph.len()).filter(|row| !hidden.contains(row)) {
            kept.set(row);
        }
        dotted_edges(&graph, &kept).dump()
    }

    #[test]
    fn same_as_intellij_simple() {
        assert_eq!(dotted("0(1)\n1(2)\n2()", &[1]), "0(2.dot)\n2()");
    }

    #[test]
    fn same_as_intellij_simple_2_up() {
        assert_eq!(
            dotted("0(2)\n1(2)\n2(3)\n3()", &[2]),
            "0(3.dot)\n1(3.dot)\n3()"
        );
    }

    #[test]
    fn same_as_intellij_simple_2_down() {
        assert_eq!(
            dotted("0(1)\n1(2, 3)\n2()\n3()", &[1]),
            "0(2.dot, 3.dot)\n2()\n3()"
        );
    }

    #[test]
    fn same_as_intellij_down_tree() {
        assert_eq!(
            dotted("0(1, 2)\n1(3, 4)\n2(4, 5)\n3()\n4()\n5()", &[1, 2]),
            "0(3.dot, 4.dot, 5.dot)\n3()\n4()\n5()"
        );
    }

    #[test]
    fn same_as_intellij_up_tree() {
        assert_eq!(
            dotted("0(3)\n1(3, 4)\n2(4)\n3(5)\n4(5)\n5()", &[3, 4]),
            "0(5.dot)\n1(5.dot)\n2(5.dot)\n5()"
        );
    }

    #[test]
    fn same_as_intellij_simple_merge() {
        assert_eq!(
            dotted("0(1, 2)\n1(3)\n2(4)\n3(5)\n4(5)\n5()", &[3, 4]),
            "0(1, 2)\n1(5.dot)\n2(5.dot)\n5()"
        );
    }

    #[test]
    fn same_as_intellij_simple_merge_2() {
        assert_eq!(
            dotted("0(1, 2)\n1(3)\n2(4)\n3(5)\n4(5)\n5()", &[1, 2]),
            "0(3.dot, 4.dot)\n3(5)\n4(5)\n5()"
        );
    }

    #[test]
    fn same_as_intellij_fork() {
        assert_eq!(
            dotted("0(2)\n1(3)\n2(3)\n3(4)\n4(5)\n5()", &[3, 4]),
            "0(2)\n1(5.dot)\n2(5.dot)\n5()"
        );
    }

    #[test]
    fn own_dotted_edge_does_not_replace_the_linear_one() {
        assert_eq!(
            dotted("0(1, 2.dot)\n1(2)\n2()", &[1]),
            "0(2.dot, 2.dot)\n2()"
        );
    }

    fn highlights(repo: &TempRepo, filter: &LogFilter) -> Vec<Highlight> {
        let store = store::load(repo.path()).unwrap();
        let me = Person {
            name: "Test".into(),
            email: "test@example.com".into(),
        };
        let highlighter = Highlighter::new(&store, Some(&me));
        let rules = highlighter.rules(&store, filter);
        (0..store.len())
            .map(|commit| highlighter.of(&store, commit, rules))
            .collect()
    }

    #[test]
    fn highlight_my_commits_only_if_there_are_more_authors() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "mine");
        assert!(!highlights(&repo, &LogFilter::default())[0].bold);

        repo.git(&[
            "-c",
            "user.name=Another",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "foreign",
        ]);

        let found = highlights(&repo, &LogFilter::default());
        assert_eq!((found[0].bold, found[1].bold), (false, true));
    }

    #[test]
    fn do_not_highlight_my_commits_if_already_filtering_by_me() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "mine");
        repo.git(&[
            "-c",
            "user.name=Another",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "foreign",
        ]);

        let filter = LogFilter {
            users: strings(&[ME]),
            ..LogFilter::default()
        };

        assert!(highlights(&repo, &filter).iter().all(|found| !found.bold));
    }

    #[test]
    fn same_person_with_another_name_format() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "mine");
        repo.git(&[
            "-c",
            "user.name=test",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "also",
        ]);

        // Both forms belong to the same person: there is still a single author.
        assert!(highlights(&repo, &LogFilter::default())
            .iter()
            .all(|found| !found.bold));
    }

    #[test]
    fn merges_in_gray() {
        let repo = repo_interleaved();

        let found = highlights(&repo, &LogFilter::default());

        assert!(found[0].muted);
        assert!(found[1..].iter().all(|found| !found.muted));
    }

    #[test]
    fn current_branch_highlights_what_it_contains() {
        let repo = repo_with_branch();
        let store = store::load(repo.path()).unwrap();
        let side = (0..store.len())
            .find(|commit| &*store.get(*commit).subject == "side fix")
            .unwrap();

        let found = highlights(&repo, &LogFilter::default());

        assert!(!found[side as usize].branch);
        assert_eq!(found.iter().filter(|found| found.branch).count(), 3);
    }

    #[test]
    fn current_branch_does_not_highlight_if_filtering_by_it_or_by_head() {
        let repo = repo_with_branch();
        for name in ["main", "HEAD"] {
            let filter = LogFilter {
                branches: strings(&[name]),
                ..LogFilter::default()
            };

            assert!(highlights(&repo, &filter).iter().all(|found| !found.branch));
        }
        let filter = LogFilter {
            branches: strings(&["other"]),
            ..LogFilter::default()
        };
        assert!(highlights(&repo, &filter).iter().any(|found| found.branch));
    }
}
