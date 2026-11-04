// Three-way line merge, ported from IntelliJ: `ComparisonMergeUtil` (crossing of the equal
// ranges), `MergeRangeUtil` (the type of each block) and `MergeResolveUtil` (automatic resolution),
// with what these use from `ByLineRt`, `ByWordRt`, `ByCharRt`, `ChunkOptimizer`, `TrimUtil` and
// `ComparisonUtil`.
//
// Decision 4 of the plan: each diff of two sequences (lines, words or punctuation marks) is `similar`'s
// Myers, not IntelliJ's "fair" diff with its line optimizations. Everything else is ported
// as is.
//
// The lines are IntelliJ's: the text split by `\n`, without the newline, so that `"x\n"`
// has two lines, `x` and the empty one. Offsets within a text are in bytes: it is only
// cut at spaces, ASCII marks and word boundaries, which are always character boundaries.
use std::borrow::Cow;
use std::collections::HashSet;
use std::hash::Hash;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use similar::{capture_diff_slices_deadline, Algorithm, DiffOp};

// `ComparisonPolicy`: `DEFAULT`, `TRIM_WHITESPACES` and `IGNORE_WHITESPACES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Whitespace {
    Default,
    Trim,
    Ignore,
}

// `MergeConflictType.Type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BlockKind {
    Inserted,
    Deleted,
    Modified,
    Conflict,
}

// A `MergeLineFragment` with its `MergeConflictType`. The ranges are of lines, half-open and
// from 0. `auto` is the text that the automatic resolution leaves in place of the lines of the
// base, joined with `\n` and without the final newline, like `getLinesContent`; only in a conflict that
// can be resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    pub left: (u32, u32),
    pub base: (u32, u32),
    pub right: (u32, u32),
    pub kind: BlockKind,
    pub left_changed: bool,
    pub right_changed: bool,
    pub auto: Option<String>,
}

// `DiffConfig.USE_GREEDY_MERGE_MAGIC_RESOLVE` in 2023.2.3.
const USE_GREEDY_MERGE_MAGIC_RESOLVE: bool = false;

// `DiffConfig.DELTA_THRESHOLD_SIZE`, without the `idea.diff.delta.threshold.size` property.
const DELTA_THRESHOLD_SIZE: usize = 20000;

// Deadline of each line diff of `blocks`. Once past, Myers ends with a valid alignment
// though not minimal: a huge file does not leave the merge window hanging.
const LINE_DIFF_DEADLINE: Duration = Duration::from_millis(500);

// Deadline of an automatic resolution. Once past, it is not resolved, as with `DiffTooBigException`.
const RESOLVE_DEADLINE: Duration = Duration::from_millis(200);

// How long all the automatic resolutions of one call to `blocks` may take together.
const RESOLVE_BUDGET: Duration = Duration::from_secs(1);

// The deadlines of a call to `blocks`: the ones above, or far-off ones in the tests.
#[derive(Debug, Clone, Copy)]
struct Limits {
    line_diff: Duration,
    resolve: Duration,
    budget: Duration,
}

const LIMITS: Limits = Limits {
    line_diff: LINE_DIFF_DEADLINE,
    resolve: RESOLVE_DEADLINE,
    budget: RESOLVE_BUDGET,
};

// `DiffTooBigException`: the comparison exceeds IntelliJ's limit or the deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TooBig;

type Checked<T> = Result<T, TooBig>;

//
// Ranges
//

// `Range`: a range of the first text and another of the second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Range {
    start1: usize,
    end1: usize,
    start2: usize,
    end2: usize,
}

impl Range {
    fn new(start1: usize, end1: usize, start2: usize, end2: usize) -> Self {
        Self {
            start1,
            end1,
            start2,
            end2,
        }
    }

    fn is_empty(&self) -> bool {
        self.start1 == self.end1 && self.start2 == self.end2
    }
}

// `MergeRange`: 1 is the left, 2 the base and 3 the right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MergeRange {
    start1: usize,
    end1: usize,
    start2: usize,
    end2: usize,
    start3: usize,
    end3: usize,
}

impl MergeRange {
    fn is_empty(&self) -> bool {
        self.start1 == self.end1 && self.start2 == self.end2 && self.start3 == self.end3
    }

    fn start(&self, side: ThreeSide) -> usize {
        match side {
            ThreeSide::Left => self.start1,
            ThreeSide::Base => self.start2,
            ThreeSide::Right => self.start3,
        }
    }

    fn end(&self, side: ThreeSide) -> usize {
        match side {
            ThreeSide::Left => self.end1,
            ThreeSide::Base => self.end2,
            ThreeSide::Right => self.end3,
        }
    }
}

// `ThreeSide`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThreeSide {
    Left,
    Base,
    Right,
}

impl ThreeSide {
    fn select<T: Copy>(self, values: [T; 3]) -> T {
        match self {
            ThreeSide::Left => values[0],
            ThreeSide::Base => values[1],
            ThreeSide::Right => values[2],
        }
    }
}

// `FairDiffIterable`: the equal ranges of two sequences, in order and not touching.
#[derive(Debug, Clone)]
struct Diff {
    len1: usize,
    len2: usize,
    unchanged: Vec<Range>,
}

impl Diff {
    // `DiffIterableUtil.createUnchanged`.
    fn from_unchanged(ranges: impl IntoIterator<Item = Range>, len1: usize, len2: usize) -> Self {
        let mut builder = EqualBuilder::new(len1, len2);
        for range in ranges {
            builder.mark_equal(range.start1, range.start2, range.end1, range.end2);
        }
        builder.finish()
    }

    // `iterateChanges`.
    fn changes(&self) -> Vec<Range> {
        let mut changes = Vec::new();
        let (mut last1, mut last2) = (0, 0);
        let end = Range::new(self.len1, self.len1, self.len2, self.len2);
        for range in self.unchanged.iter().chain([&end]) {
            if range.start1 > last1 || range.start2 > last2 {
                changes.push(Range::new(last1, range.start1, last2, range.start2));
            }
            last1 = range.end1;
            last2 = range.end2;
        }
        changes
    }
}

// `DiffIterableUtil.ChangeBuilder`: the equal ranges are marked, in order.
struct EqualBuilder {
    len1: usize,
    len2: usize,
    unchanged: Vec<Range>,
}

impl EqualBuilder {
    fn new(len1: usize, len2: usize) -> Self {
        Self {
            len1,
            len2,
            unchanged: Vec::new(),
        }
    }

    fn mark_equal(&mut self, start1: usize, start2: usize, end1: usize, end2: usize) {
        if start1 == end1 && start2 == end2 {
            return;
        }
        if let Some(last) = self.unchanged.last_mut() {
            if last.end1 == start1 && last.end2 == start2 {
                last.end1 = end1;
                last.end2 = end2;
                return;
            }
        }
        self.unchanged.push(Range::new(start1, end1, start2, end2));
    }

    fn mark_equal_one(&mut self, index1: usize, index2: usize) {
        self.mark_equal(index1, index2, index1 + 1, index2 + 1);
    }

    fn finish(self) -> Diff {
        Diff {
            len1: self.len1,
            len2: self.len2,
            unchanged: self.unchanged,
        }
    }
}

// `DiffIterableUtil.diff`. Decision 4: `similar`'s Myers instead of the "fair" diff. Once past
// `deadline`, the result is still valid though not minimal.
fn diff<T: Hash + Eq>(data1: &[T], data2: &[T], deadline: Instant) -> Diff {
    let mut builder = EqualBuilder::new(data1.len(), data2.len());
    for op in capture_diff_slices_deadline(Algorithm::Myers, data1, data2, Some(deadline)) {
        if let DiffOp::Equal {
            old_index,
            new_index,
            len,
        } = op
        {
            builder.mark_equal(old_index, new_index, old_index + len, new_index + len);
        }
    }
    builder.finish()
}

// `DiffIterableUtil.diff` with its `DiffTooBigException`. `Diff.buildChanges` removes what is common at the
// start and at the end, `Reindexer` discards what does not appear on the other side, and
// `MyersLCS.executeWithThreshold` fails if the differences of what remains exceed
// `max(20000 + 10·√(n + m), DELTA_THRESHOLD_SIZE)`. Without IntelliJ's retry with patience:
// where that would save it, here it is not resolved. It also fails if the deadline is exceeded.
fn diff_checked<T: Hash + Eq>(data1: &[T], data2: &[T], deadline: Instant) -> Checked<Diff> {
    let prefix = expand_forward(data1, data2, [0, 0], [data1.len(), data2.len()]);
    let suffix = expand_backward(data1, data2, [prefix, prefix], [data1.len(), data2.len()]);
    let trimmed1 = &data1[prefix..data1.len() - suffix];
    let trimmed2 = &data2[prefix..data2.len() - suffix];

    // `Reindexer.discardUnique`.
    let shared = |data: &[T], other: &[T]| {
        let other: HashSet<&T> = other.iter().collect();
        data.iter().filter(|item| other.contains(item)).count()
    };
    let (count1, count2) = (shared(trimmed1, trimmed2), shared(trimmed2, trimmed1));
    let limited = count1 > 0 && count2 > 0;
    let threshold =
        DELTA_THRESHOLD_SIZE.max(20000 + 10 * ((count1 + count2) as f64).sqrt() as usize);
    if limited && count1.abs_diff(count2) > threshold {
        return Err(TooBig);
    }

    let found = diff(data1, data2, deadline);
    if Instant::now() > deadline {
        return Err(TooBig);
    }
    let matched: usize = found
        .unchanged
        .iter()
        .map(|range| range.end1 - range.start1)
        .sum::<usize>()
        .saturating_sub(prefix + suffix);
    let differences = (count1 + count2).saturating_sub(2 * matched);
    if limited && differences > threshold {
        return Err(TooBig);
    }
    Ok(found)
}

//
// ComparisonMergeUtil
//

// `ComparisonMergeUtil.SideEquality`: whether the lines of the three sides are really equal.
type SideEquality<'a> = &'a dyn Fn(usize, usize, usize) -> bool;

// `ComparisonMergeUtil.buildSimple`: `fragments1` goes from the base to the left and `fragments2` from
// the base to the right.
fn build_simple(fragments1: &Diff, fragments2: &Diff) -> Vec<MergeRange> {
    FairMergeBuilder::new(None).execute(fragments1, fragments2)
}

// `ComparisonMergeUtil.buildMerge`: it also separates the changes ignored by the policy.
fn build_merge(
    fragments1: &Diff,
    fragments2: &Diff,
    true_equality: SideEquality<'_>,
) -> Vec<MergeRange> {
    FairMergeBuilder::new(Some(true_equality)).execute(fragments1, fragments2)
}

// `ComparisonMergeUtil.FairMergeBuilder`: what is equal in both diffs at once does not change;
// what is left between two of those ranges is a block.
struct FairMergeBuilder<'a> {
    changes_builder: ChangeBuilder<'a>,
}

impl<'a> FairMergeBuilder<'a> {
    fn new(true_equality: Option<SideEquality<'a>>) -> Self {
        Self {
            changes_builder: ChangeBuilder::new(true_equality),
        }
    }

    fn execute(mut self, fragments1: &Diff, fragments2: &Diff) -> Vec<MergeRange> {
        debug_assert_eq!(fragments1.len1, fragments2.len1);
        let (mut index1, mut index2) = (0, 0);
        while index1 < fragments1.unchanged.len() && index2 < fragments2.unchanged.len() {
            let left = self.add(fragments1.unchanged[index1], fragments2.unchanged[index2]);
            if left {
                index1 += 1;
            } else {
                index2 += 1;
            }
        }
        self.changes_builder
            .finish(fragments1.len2, fragments1.len1, fragments2.len2)
    }

    // Returns whether it advances in the first diff (`Side.LEFT`).
    fn add(&mut self, range1: Range, range2: Range) -> bool {
        let (start1, end1) = (range1.start1, range1.end1);
        let (start2, end2) = (range2.start1, range2.end1);

        if end1 <= start2 {
            return true;
        }
        if end2 <= start1 {
            return false;
        }

        let start_base = start1.max(start2);
        let end_base = end1.min(end2);
        let count = end_base - start_base;

        let start_shift1 = start_base - start1;
        let start_shift2 = start_base - start2;

        let start_left = range1.start2 + start_shift1;
        let end_left = start_left + count;
        let start_right = range2.start2 + start_shift2;
        let end_right = start_right + count;

        self.changes_builder.mark_equal(
            start_left,
            start_base,
            start_right,
            end_left,
            end_base,
            end_right,
        );

        end1 <= end2
    }
}

// `ComparisonMergeUtil.ChangeBuilder`, and with `true_equality` `IgnoringChangeBuilder`: within
// the ranges equal according to the policy, the lines that are not really equal form separate
// blocks, one per consecutive run.
struct ChangeBuilder<'a> {
    changes: Vec<MergeRange>,
    index1: usize,
    index2: usize,
    index3: usize,
    true_equality: Option<SideEquality<'a>>,
}

impl<'a> ChangeBuilder<'a> {
    fn new(true_equality: Option<SideEquality<'a>>) -> Self {
        Self {
            changes: Vec::new(),
            index1: 0,
            index2: 0,
            index3: 0,
            true_equality,
        }
    }

    fn add_change(
        &mut self,
        start1: usize,
        start2: usize,
        start3: usize,
        end1: usize,
        end2: usize,
        end3: usize,
    ) {
        if start1 == end1 && start2 == end2 && start3 == end3 {
            return;
        }
        self.changes.push(MergeRange {
            start1,
            end1,
            start2,
            end2,
            start3,
            end3,
        });
    }

    fn mark_equal(
        &mut self,
        start1: usize,
        start2: usize,
        start3: usize,
        end1: usize,
        end2: usize,
        end3: usize,
    ) {
        debug_assert!(self.index1 <= start1 && self.index2 <= start2 && self.index3 <= start3);
        self.process_change(
            self.index1,
            self.index2,
            self.index3,
            start1,
            start2,
            start3,
        );
        self.index1 = end1;
        self.index2 = end2;
        self.index3 = end3;
    }

    fn finish(mut self, length1: usize, length2: usize, length3: usize) -> Vec<MergeRange> {
        self.process_change(
            self.index1,
            self.index2,
            self.index3,
            length1,
            length2,
            length3,
        );
        self.changes
    }

    fn process_change(
        &mut self,
        start1: usize,
        start2: usize,
        start3: usize,
        end1: usize,
        end2: usize,
        end3: usize,
    ) {
        // `IgnoringChangeBuilder.processChange`: before the change, what was ignored since the previous one.
        if let Some(true_equality) = self.true_equality {
            let (unchanged1, unchanged2, unchanged3) = self
                .changes
                .last()
                .map_or((0, 0, 0), |last| (last.end1, last.end2, last.end3));
            self.add_ignored_changes(
                true_equality,
                [unchanged1, unchanged2, unchanged3],
                [start1, start2, start3],
            );
        }
        self.add_change(start1, start2, start3, end1, end2, end3);
    }

    // `IgnoringChangeBuilder.addIgnoredChanges`.
    fn add_ignored_changes(
        &mut self,
        true_equality: SideEquality<'_>,
        [start1, start2, start3]: [usize; 3],
        [end1, end2, end3]: [usize; 3],
    ) {
        // It is an equal range: the three measure the same (IntelliJ checks it with `assert`).
        let count = (end2 - start2).min(end1 - start1).min(end3 - start3);
        let mut first_ignored: Option<usize> = None;
        for i in 0..count {
            let is_ignored = !true_equality(start1 + i, start2 + i, start3 + i);
            match (is_ignored, first_ignored) {
                (true, None) => first_ignored = Some(i),
                (false, Some(first)) => {
                    self.add_change(
                        start1 + first,
                        start2 + first,
                        start3 + first,
                        start1 + i,
                        start2 + i,
                        start3 + i,
                    );
                    first_ignored = None;
                }
                _ => {}
            }
        }
        if let Some(first) = first_ignored {
            self.add_change(
                start1 + first,
                start2 + first,
                start3 + first,
                start1 + count,
                start2 + count,
                start3 + count,
            );
        }
    }
}

// `ComparisonMergeUtil.tryResolveConflict`: the variant chosen by `DiffConfig`.
fn try_resolve_conflict(left: &str, base: &str, right: &str, deadline: Instant) -> Option<String> {
    if USE_GREEDY_MERGE_MAGIC_RESOLVE {
        try_greedy_resolve_until(left, base, right, deadline)
    } else {
        try_resolve_until(left, base, right, deadline)
    }
}

//
// MergeRangeUtil
//

// `MergeConflictType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MergeConflictType {
    kind: BlockKind,
    left_change: bool,
    right_change: bool,
    can_be_resolved: bool,
}

impl MergeConflictType {
    fn new(kind: BlockKind, left_change: bool, right_change: bool) -> Self {
        Self::resolvable(kind, left_change, right_change, true)
    }

    fn resolvable(kind: BlockKind, left_change: bool, right_change: bool, can: bool) -> Self {
        Self {
            kind,
            left_change,
            right_change,
            can_be_resolved: can,
        }
    }
}

// `MergeRangeUtil.getMergeType`.
fn get_merge_type(
    emptiness: impl Fn(ThreeSide) -> bool,
    equality: impl Fn(ThreeSide, ThreeSide) -> bool,
    true_equality: Option<&dyn Fn(ThreeSide, ThreeSide) -> bool>,
    conflict_resolver: impl FnOnce() -> bool,
) -> MergeConflictType {
    use BlockKind::{Conflict, Deleted, Inserted, Modified};
    use ThreeSide::{Base, Left, Right};

    let is_left_empty = emptiness(Left);
    let is_base_empty = emptiness(Base);
    let is_right_empty = emptiness(Right);
    debug_assert!(!is_left_empty || !is_base_empty || !is_right_empty);

    if is_base_empty {
        if is_left_empty {
            // --=
            return MergeConflictType::new(Inserted, false, true);
        }
        if is_right_empty {
            // =--
            return MergeConflictType::new(Inserted, true, false);
        }
        // =-=
        if equality(Left, Right) {
            return MergeConflictType::new(Inserted, true, true);
        }
        return MergeConflictType::resolvable(Conflict, true, true, false);
    }

    if is_left_empty && is_right_empty {
        // -=-
        return MergeConflictType::new(Deleted, true, true);
    }

    // -==, ==-, ===
    let unchanged_left = equality(Base, Left);
    let unchanged_right = equality(Base, Right);

    if unchanged_left && unchanged_right {
        // Only gets here with `trueEquality`: equal according to the policy, really different.
        let true_equality = true_equality.unwrap_or(&equality);
        let true_unchanged_left = true_equality(Base, Left);
        let true_unchanged_right = true_equality(Base, Right);
        return MergeConflictType::new(Modified, !true_unchanged_left, !true_unchanged_right);
    }

    if unchanged_left {
        let kind = if is_right_empty { Deleted } else { Modified };
        return MergeConflictType::new(kind, false, true);
    }
    if unchanged_right {
        let kind = if is_left_empty { Deleted } else { Modified };
        return MergeConflictType::new(kind, true, false);
    }

    if equality(Left, Right) {
        return MergeConflictType::new(Modified, true, true);
    }
    let can_be_resolved = !is_left_empty && !is_right_empty && conflict_resolver();
    MergeConflictType::resolvable(Conflict, true, true, can_be_resolved)
}

// `MergeRangeUtil.compareLineMergeContents`.
fn compare_line_merge_contents(
    fragment: &MergeRange,
    lines: [&[&str]; 3],
    policy: Whitespace,
    side1: ThreeSide,
    side2: ThreeSide,
) -> bool {
    let (start1, end1) = (fragment.start(side1), fragment.end(side1));
    let (start2, end2) = (fragment.start(side2), fragment.end(side2));
    if end2 - start2 != end1 - start1 {
        return false;
    }
    let lines1 = &side1.select(lines)[start1..end1];
    let lines2 = &side2.select(lines)[start2..end2];
    lines1
        .iter()
        .zip(lines2)
        .all(|(line1, line2)| is_equal_texts(line1.as_bytes(), line2.as_bytes(), policy))
}

// `DiffRangeUtil.getLinesContent`: the lines joined, without the newline of the last one.
fn lines_content(lines: &[&str], start: usize, end: usize) -> String {
    lines[start..end].join("\n")
}

// `MergeRangeUtil.getLineMergeType` and `canResolveLineConflict`, with the resolved text.
fn line_block(
    fragment: MergeRange,
    lines: [&[&str]; 3],
    policy: Whitespace,
    resolve: Duration,
    resolve_budget: Instant,
) -> Block {
    let mut auto = None;
    let found = get_merge_type(
        |side| fragment.start(side) == fragment.end(side),
        |side1, side2| compare_line_merge_contents(&fragment, lines, policy, side1, side2),
        Some(&|side1: ThreeSide, side2: ThreeSide| {
            compare_line_merge_contents(&fragment, lines, Whitespace::Default, side1, side2)
        }),
        || {
            let content = |side: ThreeSide| {
                lines_content(side.select(lines), fragment.start(side), fragment.end(side))
            };
            let deadline = (Instant::now() + resolve).min(resolve_budget);
            auto = try_resolve_conflict(
                &content(ThreeSide::Left),
                &content(ThreeSide::Base),
                &content(ThreeSide::Right),
                deadline,
            );
            auto.is_some()
        },
    );
    let pair = |start: usize, end: usize| (start as u32, end as u32);
    Block {
        left: pair(fragment.start1, fragment.end1),
        base: pair(fragment.start2, fragment.end2),
        right: pair(fragment.start3, fragment.end3),
        kind: found.kind,
        left_changed: found.left_change,
        right_changed: found.right_change,
        auto: auto.filter(|_| found.kind == BlockKind::Conflict && found.can_be_resolved),
    }
}

//
// ByLineRt
//

fn split_lines(text: &str) -> Vec<&str> {
    text.split('\n').collect()
}

// What is compared of a line according to the policy (`ByLineRt.Line`, `ComparisonUtil.isEquals`).
fn line_keys<'a>(lines: &[&'a str], policy: Whitespace) -> Vec<Cow<'a, str>> {
    let key = |line: &'a str| match policy {
        Whitespace::Default => Cow::Borrowed(line),
        Whitespace::Trim => Cow::Borrowed(line.trim_matches(is_white_space_char)),
        Whitespace::Ignore => {
            Cow::Owned(line.chars().filter(|c| !is_white_space_char(*c)).collect())
        }
    };
    lines.iter().map(|line| key(line)).collect()
}

// `ByLineRt.doCompare` with three texts. Decision 4: a Myers from the base to each side over the
// lines according to the policy, instead of `compareSmart`, `optimizeLineChunks` and
// `correctChangesSecondStep`. With `keep_ignored_changes` (`ByLineRt.merge`) the changes that the
// policy ignores are left as separate blocks.
fn compare_line_ranges(
    lines1: &[&str],
    lines2: &[&str],
    lines3: &[&str],
    policy: Whitespace,
    keep_ignored_changes: bool,
    line_diff: Duration,
) -> Vec<MergeRange> {
    let keys1 = line_keys(lines1, policy);
    let keys2 = line_keys(lines2, policy);
    let keys3 = line_keys(lines3, policy);
    let iterable1 = diff(&keys2, &keys1, Instant::now() + line_diff);
    let iterable2 = diff(&keys2, &keys3, Instant::now() + line_diff);

    if keep_ignored_changes && policy != Whitespace::Default {
        // `ByLineRt.equalsDefaultPolicy`.
        let equals = |index1: usize, index2: usize, index3: usize| {
            lines2[index2] == lines1[index1] && lines2[index2] == lines3[index3]
        };
        build_merge(&iterable1, &iterable2, &equals)
    } else {
        build_simple(&iterable1, &iterable2)
    }
}

// `ComparisonManagerImpl.mergeLines` and the type of each block, like `MergeThreesideViewer`.
pub fn blocks(left: &str, base: &str, right: &str, policy: Whitespace) -> Vec<Block> {
    blocks_within(left, base, right, policy, LIMITS)
}

fn blocks_within(
    left: &str,
    base: &str,
    right: &str,
    policy: Whitespace,
    limits: Limits,
) -> Vec<Block> {
    let (left, base, right) = (split_lines(left), split_lines(base), split_lines(right));
    let lines: [&[&str]; 3] = [&left, &base, &right];
    let ranges = compare_line_ranges(&left, &base, &right, policy, true, limits.line_diff);
    let resolve_budget = Instant::now() + limits.budget;
    ranges
        .into_iter()
        .map(|fragment| line_block(fragment, lines, policy, limits.resolve, resolve_budget))
        .collect()
}

//
// ComparisonUtil and TrimUtil
//

// `Strings.isWhiteSpace`: only space, tab and newline.
fn is_white_space(c: u8) -> bool {
    c == b'\n' || c == b'\t' || c == b' '
}

fn is_white_space_char(c: char) -> bool {
    c == '\n' || c == '\t' || c == ' '
}

// `TrimUtil.isPunctuation`.
fn is_punctuation(b: u32) -> bool {
    if b == 95 {
        return false; // '_'
    }
    (33..=47).contains(&b) // !"#$%&'()*+,-./
        || (58..=64).contains(&b) // :;<=>?@
        || (91..=96).contains(&b) // [\]^_`
        || (123..=126).contains(&b) // {|}~
}

// `TrimUtil.isAlpha`.
fn is_alpha(c: char) -> bool {
    if c.is_ascii() && is_white_space(c as u8) {
        return false;
    }
    !is_punctuation(c as u32)
}

// `TrimUtil.isContinuousScript`. No Unicode tables in the standard library: `isDigit` is
// approximated with `is_numeric`, `isIdeographic` and the writing systems with their blocks.
fn is_continuous_script(c: char) -> bool {
    let code = c as u32;
    if code < 128 {
        return false;
    }
    if c.is_numeric() {
        return false;
    }
    if code > 0xFFFF {
        return true; // `!Character.isBmpCodePoint`
    }
    let ideographic = matches!(code,
        0x3006 | 0x3007 | 0x3021..=0x3029 | 0x3038..=0x303A | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF | 0xF900..=0xFAFF);
    if ideographic || !c.is_alphabetic() {
        return true;
    }
    matches!(code,
        // HIRAGANA
        0x3041..=0x3096 | 0x309D..=0x309F
        // KATAKANA
        | 0x30A1..=0x30FA | 0x30FD..=0x30FF | 0x31F0..=0x31FF | 0x32D0..=0x32FE
        | 0x3300..=0x3357 | 0xFF66..=0xFF6F | 0xFF71..=0xFF9D
        // THAI
        | 0x0E01..=0x0E3A | 0x0E40..=0x0E5B
        // JAVANESE
        | 0xA980..=0xA9CD | 0xA9D0..=0xA9D9 | 0xA9DE..=0xA9DF)
}

fn trim_start(text: &[u8], mut start: usize, end: usize) -> usize {
    while start < end && is_white_space(text[start]) {
        start += 1;
    }
    start
}

fn trim_end(text: &[u8], start: usize, mut end: usize) -> usize {
    while start < end && is_white_space(text[end - 1]) {
        end -= 1;
    }
    end
}

fn trim_bytes(text: &[u8]) -> &[u8] {
    let start = trim_start(text, 0, text.len());
    &text[start..trim_end(text, start, text.len())]
}

// `Strings.equalsIgnoreWhitespaces`.
fn equals_ignore_whitespaces(text1: &[u8], text2: &[u8]) -> bool {
    let kept = |text: &[u8]| -> Vec<u8> {
        text.iter()
            .copied()
            .filter(|c| !is_white_space(*c))
            .collect()
    };
    kept(text1) == kept(text2)
}

// `ComparisonUtil.equalsTrimWhitespaces`: also without spaces at the sides of each line.
fn equals_trim_whitespaces(text1: &[u8], text2: &[u8]) -> bool {
    let lines1: Vec<&[u8]> = text1.split(|c| *c == b'\n').collect();
    let lines2: Vec<&[u8]> = text2.split(|c| *c == b'\n').collect();
    lines1.len() == lines2.len()
        && lines1
            .iter()
            .zip(&lines2)
            .all(|(line1, line2)| trim_bytes(line1) == trim_bytes(line2))
}

// `ComparisonUtil.isEqualTexts`.
fn is_equal_texts(text1: &[u8], text2: &[u8], policy: Whitespace) -> bool {
    match policy {
        Whitespace::Default => text1 == text2,
        Whitespace::Trim => equals_trim_whitespaces(text1, text2),
        Whitespace::Ignore => equals_ignore_whitespaces(text1, text2),
    }
}

// `TrimUtil.expandWhitespacesForward` (`expandIgnoredForward` with spaces of `text1`).
fn expand_whitespaces_forward(
    texts: [&[u8]; 2],
    [mut start1, mut start2]: [usize; 2],
    [end1, end2]: [usize; 2],
) -> usize {
    let [text1, text2] = texts;
    let old_start1 = start1;
    while start1 < end1 && start2 < end2 {
        if text1[start1] != text2[start2] || !is_white_space(text1[start1]) {
            break;
        }
        start1 += 1;
        start2 += 1;
    }
    start1 - old_start1
}

// `TrimUtil.expandWhitespacesBackward`.
fn expand_whitespaces_backward(
    texts: [&[u8]; 2],
    [start1, start2]: [usize; 2],
    [mut end1, mut end2]: [usize; 2],
) -> usize {
    let [text1, text2] = texts;
    let old_end1 = end1;
    while start1 < end1 && start2 < end2 {
        if text1[end1 - 1] != text2[end2 - 1] || !is_white_space(text1[end1 - 1]) {
            break;
        }
        end1 -= 1;
        end2 -= 1;
    }
    old_end1 - end1
}

// `TrimUtil.expandWhitespacesForward` with three texts.
fn expand_whitespaces_forward3(
    texts: [&[u8]; 3],
    [mut start1, mut start2, mut start3]: [usize; 3],
    [end1, end2, end3]: [usize; 3],
) -> usize {
    let [text1, text2, text3] = texts;
    let old_start1 = start1;
    while start1 < end1 && start2 < end2 && start3 < end3 {
        let c = text1[start1];
        if c != text2[start2] || c != text3[start3] || !is_white_space(c) {
            break;
        }
        start1 += 1;
        start2 += 1;
        start3 += 1;
    }
    start1 - old_start1
}

// `TrimUtil.expandWhitespacesBackward` with three texts.
fn expand_whitespaces_backward3(
    texts: [&[u8]; 3],
    [start1, start2, start3]: [usize; 3],
    [mut end1, mut end2, mut end3]: [usize; 3],
) -> usize {
    let [text1, text2, text3] = texts;
    let old_end1 = end1;
    while start1 < end1 && start2 < end2 && start3 < end3 {
        let c = text1[end1 - 1];
        if c != text2[end2 - 1] || c != text3[end3 - 1] || !is_white_space(c) {
            break;
        }
        end1 -= 1;
        end2 -= 1;
        end3 -= 1;
    }
    old_end1 - end1
}

// `TrimUtil.expandWhitespaces` (`expandIgnored`).
fn expand_whitespaces(texts: [&[u8]; 2], range: Range) -> Range {
    let mut starts = [range.start1, range.start2];
    let mut ends = [range.end1, range.end2];
    let count1 = expand_whitespaces_forward(texts, starts, ends);
    starts = starts.map(|start| start + count1);
    let count2 = expand_whitespaces_backward(texts, starts, ends);
    ends = ends.map(|end| end - count2);
    Range::new(starts[0], ends[0], starts[1], ends[1])
}

fn expand_whitespaces3(texts: [&[u8]; 3], range: MergeRange) -> MergeRange {
    let mut starts = [range.start1, range.start2, range.start3];
    let mut ends = [range.end1, range.end2, range.end3];
    let count1 = expand_whitespaces_forward3(texts, starts, ends);
    starts = starts.map(|start| start + count1);
    let count2 = expand_whitespaces_backward3(texts, starts, ends);
    ends = ends.map(|end| end - count2);
    merge_range(starts, ends)
}

fn merge_range([start1, start2, start3]: [usize; 3], [end1, end2, end3]: [usize; 3]) -> MergeRange {
    MergeRange {
        start1,
        end1,
        start2,
        end2,
        start3,
        end3,
    }
}

// `TrimUtil.trim` with two and with three texts.
fn trim2(texts: [&[u8]; 2], range: Range) -> Range {
    let start1 = trim_start(texts[0], range.start1, range.end1);
    let end1 = trim_end(texts[0], start1, range.end1);
    let start2 = trim_start(texts[1], range.start2, range.end2);
    let end2 = trim_end(texts[1], start2, range.end2);
    Range::new(start1, end1, start2, end2)
}

fn trim3(texts: [&[u8]; 3], range: MergeRange) -> MergeRange {
    let side = |text: &[u8], start: usize, end: usize| {
        let start = trim_start(text, start, end);
        (start, trim_end(text, start, end))
    };
    let (start1, end1) = side(texts[0], range.start1, range.end1);
    let (start2, end2) = side(texts[1], range.start2, range.end2);
    let (start3, end3) = side(texts[2], range.start3, range.end3);
    merge_range([start1, start2, start3], [end1, end2, end3])
}

// `TrimUtil.isEqualsIgnoreWhitespaces` with two and with three texts.
fn is_equals_ignore_whitespaces2(texts: [&[u8]; 2], range: Range) -> bool {
    equals_ignore_whitespaces(
        &texts[0][range.start1..range.end1],
        &texts[1][range.start2..range.end2],
    )
}

fn is_equals_ignore_whitespaces3(texts: [&[u8]; 3], range: MergeRange) -> bool {
    let base = &texts[1][range.start2..range.end2];
    equals_ignore_whitespaces(base, &texts[0][range.start1..range.end1])
        && equals_ignore_whitespaces(base, &texts[2][range.start3..range.end3])
}

//
// ByWordRt
//

// `ByWordRt.InlineChunk`: a word (`WordChunk`) or a newline (`NewlineChunk`).
#[derive(Debug, Clone, Copy)]
struct InlineChunk {
    offset1: usize,
    offset2: usize,
    newline: bool,
}

// What is compared of each chunk: the text of a word, or that it is a newline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ChunkKey<'a> {
    Word(&'a [u8]),
    Newline,
}

// A text split into chunks, with what is compared of each one.
struct Words<'a> {
    text: &'a [u8],
    chunks: Vec<InlineChunk>,
    keys: Vec<ChunkKey<'a>>,
}

impl<'a> Words<'a> {
    fn new(text: &'a str) -> Self {
        let chunks = inline_chunks(text);
        let bytes = text.as_bytes();
        let keys = chunks
            .iter()
            .map(|chunk| {
                if chunk.newline {
                    ChunkKey::Newline
                } else {
                    ChunkKey::Word(&bytes[chunk.offset1..chunk.offset2])
                }
            })
            .collect();
        Self {
            text: bytes,
            chunks,
            keys,
        }
    }
}

// `ByWordRt.getInlineChunks`.
fn inline_chunks(text: &str) -> Vec<InlineChunk> {
    let word = |offset1: usize, offset2: usize| InlineChunk {
        offset1,
        offset2,
        newline: false,
    };
    let mut chunks = Vec::new();
    let mut word_start: Option<usize> = None;

    for (offset, ch) in text.char_indices() {
        let alpha = is_alpha(ch);
        let word_part = alpha && !is_continuous_script(ch);

        if word_part {
            word_start.get_or_insert(offset);
            continue;
        }
        if let Some(start) = word_start.take() {
            chunks.push(word(start, offset));
        }
        if alpha {
            // continuous script
            chunks.push(word(offset, offset + ch.len_utf8()));
        } else if ch == '\n' {
            chunks.push(InlineChunk {
                offset1: offset,
                offset2: offset + 1,
                newline: true,
            });
        }
    }
    if let Some(start) = word_start {
        chunks.push(word(start, text.len()));
    }
    chunks
}

// `ByWordRt.compare` with two texts: the changes from `text1` to `text2`. `MergeResolveUtil` only
// asks for it with `DEFAULT` or `IGNORE_WHITESPACES`.
fn compare_words(
    text1: &str,
    text2: &str,
    policy: Whitespace,
    deadline: Instant,
) -> Checked<Vec<Range>> {
    let (words1, words2) = (Words::new(text1), Words::new(text2));
    let word_changes = diff_checked(&words1.keys, &words2.keys, deadline)?;
    let word_changes = optimize_word_chunks(&words1, &words2, &word_changes);
    let delimiters = match_adjustment_delimiters(&words1, &words2, &word_changes, deadline)?;
    let texts = [words1.text, words2.text];
    Ok(if policy == Whitespace::Ignore {
        ignore_spaces_corrector(texts, &delimiters)
    } else {
        default_corrector(texts, &delimiters)
    })
}

// `ByWordRt.compare` with three texts: left, base and right.
fn compare_words3(
    [text1, text2, text3]: [&str; 3],
    policy: Whitespace,
    deadline: Instant,
) -> Checked<Vec<MergeRange>> {
    let (words1, words2, words3) = (Words::new(text1), Words::new(text2), Words::new(text3));

    let word_changes1 = diff_checked(&words2.keys, &words1.keys, deadline)?;
    let word_changes1 = optimize_word_chunks(&words2, &words1, &word_changes1);
    let iterable1 = match_adjustment_delimiters(&words2, &words1, &word_changes1, deadline)?;

    let word_changes2 = diff_checked(&words2.keys, &words3.keys, deadline)?;
    let word_changes2 = optimize_word_chunks(&words2, &words3, &word_changes2);
    let iterable2 = match_adjustment_delimiters(&words2, &words3, &word_changes2, deadline)?;

    let word_conflicts = build_simple(&iterable1, &iterable2);
    let texts = [words1.text, words2.text, words3.text];
    Ok(if policy == Whitespace::Ignore {
        merge_ignore_spaces_corrector(texts, &word_conflicts)
    } else {
        merge_default_corrector(texts, &word_conflicts)
    })
}

// `ChunkOptimizer.WordChunkOptimizer`: joins equal chunks separated by a different one and, if that
// is not possible, moves the edge to a space to change the smallest number of "phrases".
fn optimize_word_chunks(words1: &Words<'_>, words2: &Words<'_>, iterable: &Diff) -> Diff {
    let mut ranges: Vec<Range> = Vec::new();
    for range in &iterable.unchanged {
        ranges.push(*range);
        process_last_ranges(words1, words2, &mut ranges);
    }
    Diff::from_unchanged(ranges, words1.chunks.len(), words2.chunks.len())
}

// `ChunkOptimizer.processLastRanges`; the recursion is the loop.
fn process_last_ranges(words1: &Words<'_>, words2: &Words<'_>, ranges: &mut Vec<Range>) {
    let (keys1, keys2) = (&words1.keys, &words2.keys);
    loop {
        let size = ranges.len();
        if size < 2 {
            return;
        }
        let range1 = ranges[size - 2];
        let range2 = ranges[size - 1];
        if range1.end1 != range2.start1 && range1.end2 != range2.start2 {
            return;
        }

        let count1 = range1.end1 - range1.start1;
        let count2 = range2.end1 - range2.start1;

        let equal_forward = expand_forward(
            keys1,
            keys2,
            [range1.end1, range1.end2],
            [range1.end1 + count2, range1.end2 + count2],
        );
        let equal_backward = expand_backward(
            keys1,
            keys2,
            [range2.start1 - count1, range2.start2 - count1],
            [range2.start1, range2.start2],
        );

        if equal_forward == 0 && equal_backward == 0 {
            return;
        }

        // [A]B[B] -> [AB]B
        if equal_forward == count2 {
            ranges.truncate(size - 2);
            ranges.push(Range::new(
                range1.start1,
                range1.end1 + count2,
                range1.start2,
                range1.end2 + count2,
            ));
            continue;
        }

        // [A]A[B] -> A[AB]
        if equal_backward == count1 {
            ranges.truncate(size - 2);
            ranges.push(Range::new(
                range2.start1 - count1,
                range2.end1,
                range2.start2 - count1,
                range2.end2,
            ));
            continue;
        }

        let touch_left = range1.end1 == range2.start1;
        let shift = word_shift(
            words1,
            words2,
            touch_left,
            equal_forward,
            equal_backward,
            range2,
        );
        if shift != 0 {
            let moved = |index: usize| index.saturating_add_signed(shift);
            ranges.truncate(size - 2);
            ranges.push(Range::new(
                range1.start1,
                moved(range1.end1),
                range1.start2,
                moved(range1.end2),
            ));
            ranges.push(Range::new(
                moved(range2.start1),
                range2.end1,
                moved(range2.start2),
                range2.end2,
            ));
        }
        return;
    }
}

// `TrimUtil.expandForward` over two lists.
fn expand_forward<T: PartialEq>(
    data1: &[T],
    data2: &[T],
    [mut start1, mut start2]: [usize; 2],
    [end1, end2]: [usize; 2],
) -> usize {
    let old_start1 = start1;
    while start1 < end1 && start2 < end2 && data1[start1] == data2[start2] {
        start1 += 1;
        start2 += 1;
    }
    start1 - old_start1
}

// `TrimUtil.expandBackward` over two lists.
fn expand_backward<T: PartialEq>(
    data1: &[T],
    data2: &[T],
    [start1, start2]: [usize; 2],
    [mut end1, mut end2]: [usize; 2],
) -> usize {
    let old_end1 = end1;
    while start1 < end1 && start2 < end2 && data1[end1 - 1] == data2[end2 - 1] {
        end1 -= 1;
        end2 -= 1;
    }
    old_end1 - end1
}

// `WordChunkOptimizer.getShift`: 0 moves nothing, positive forward and negative backward.
fn word_shift(
    words1: &Words<'_>,
    words2: &Words<'_>,
    touch_left: bool,
    equal_forward: usize,
    equal_backward: usize,
    range2: Range,
) -> isize {
    let (touch, touch_start) = if touch_left {
        (words1, range2.start1)
    } else {
        (words2, range2.start2)
    };
    let (text, chunks) = (touch.text, &touch.chunks);
    if touch_start == 0 {
        return 0; // Two ranges that touch: not possible.
    }

    // Already separated by spaces.
    if is_separated_with_whitespace(text, chunks[touch_start - 1], chunks[touch_start]) {
        return 0;
    }

    // [X]A Y[A ZA] -> [XA] YA [ZA]
    if let Some(shift) = find_sequence_edge_shift(text, chunks, touch_start, equal_forward, true) {
        return shift as isize;
    }

    // [AX A]Y A[Z] -> [AX] AY [AZ]
    if let Some(shift) =
        find_sequence_edge_shift(text, chunks, touch_start - 1, equal_backward, false)
    {
        return -(shift as isize);
    }

    0
}

// `WordChunkOptimizer.findSequenceEdgeShift`: `None` for IntelliJ's -1.
fn find_sequence_edge_shift(
    text: &[u8],
    words: &[InlineChunk],
    offset: usize,
    count: usize,
    left_to_right: bool,
) -> Option<usize> {
    (0..count)
        .find(|i| {
            let (word1, word2) = if left_to_right {
                (words[offset + i], words[offset + i + 1])
            } else {
                (words[offset - i - 1], words[offset - i])
            };
            is_separated_with_whitespace(text, word1, word2)
        })
        .map(|i| i + 1)
}

// `WordChunkOptimizer.isSeparatedWithWhitespace`.
fn is_separated_with_whitespace(text: &[u8], word1: InlineChunk, word2: InlineChunk) -> bool {
    if word1.newline || word2.newline {
        return true;
    }
    text[word1.offset2..word2.offset1]
        .iter()
        .any(|c| is_white_space(*c))
}

// `ByWordRt.matchAdjustmentDelimiters`: from words to bytes, also matching the punctuation
// marks of the gaps between equal words.
fn match_adjustment_delimiters(
    words1: &Words<'_>,
    words2: &Words<'_>,
    changes: &Diff,
    deadline: Instant,
) -> Checked<Diff> {
    let mut matcher = AdjustmentPunctuationMatcher {
        words1,
        words2,
        builder: EqualBuilder::new(words1.text.len(), words2.text.len()),
        last: None,
        deadline,
    };
    matcher.execute(changes)?;
    Ok(matcher.builder.finish())
}

// `ByWordRt.AdjustmentPunctuationMatcher`. For example, "[ X { A ! B } Y ]" against
// "( X ... Y )" compares three groups of marks: "[" with "(", "{" + "}" with "..." and "]" with ")".
struct AdjustmentPunctuationMatcher<'a, 'b> {
    words1: &'a Words<'b>,
    words2: &'a Words<'b>,
    builder: EqualBuilder,
    // `lastStart1`, `lastStart2`, `lastEnd1` and `lastEnd2`; `None` is IntelliJ's -1.
    last: Option<[usize; 4]>,
    deadline: Instant,
}

impl AdjustmentPunctuationMatcher<'_, '_> {
    fn execute(&mut self, changes: &Diff) -> Checked<()> {
        self.last = None;
        self.match_forward(None, None);

        for ch in &changes.unchanged {
            for i in 0..ch.end1 - ch.start1 {
                let index1 = ch.start1 + i;
                let index2 = ch.start2 + i;

                let start1 = self.words1.chunks[index1].offset1;
                let start2 = self.words2.chunks[index2].offset1;
                let end1 = self.words1.chunks[index1].offset2;
                let end2 = self.words2.chunks[index2].offset2;

                self.match_backward(index1, index2)?;
                self.builder.mark_equal(start1, start2, end1, end2);
                self.match_forward(Some(index1), Some(index2));
            }
        }

        self.match_backward(self.words1.chunks.len(), self.words2.chunks.len())
    }

    // The gap before the word `index`.
    fn match_backward(&mut self, index1: usize, index2: usize) -> Checked<()> {
        let gap = |words: &Words<'_>, index: usize| {
            let start = if index == 0 {
                0
            } else {
                words.chunks[index - 1].offset2
            };
            let end = match words.chunks.get(index) {
                Some(chunk) => chunk.offset1,
                None => words.text.len(),
            };
            (start, end)
        };
        let (start1, end1) = gap(self.words1, index1);
        let (start2, end2) = gap(self.words2, index2);
        self.match_backward_range(start1, start2, end1, end2)?;
        self.last = None;
        Ok(())
    }

    // The gap after the word `index`; `None` is before the first one.
    fn match_forward(&mut self, index1: Option<usize>, index2: Option<usize>) {
        let gap = |words: &Words<'_>, index: Option<usize>| {
            let start = index.map_or(0, |index| words.chunks[index].offset2);
            let next = index.map_or(0, |index| index + 1);
            let end = match words.chunks.get(next) {
                Some(chunk) => chunk.offset1,
                None => words.text.len(),
            };
            (start, end)
        };
        let (start1, end1) = gap(self.words1, index1);
        let (start2, end2) = gap(self.words2, index2);
        debug_assert!(self.last.is_none());
        self.last = Some([start1, start2, end1, end2]);
    }

    fn match_backward_range(
        &mut self,
        start1: usize,
        start2: usize,
        end1: usize,
        end2: usize,
    ) -> Checked<()> {
        let Some([last_start1, last_start2, last_end1, last_end2]) = self.last else {
            return Ok(());
        };

        // Two equal words in a row: the gap between them is matched ("A B" - "A B").
        if last_start1 == start1 && last_start2 == start2 {
            return self.match_range(start1, start2, end1, end2);
        }
        // Two equal words with other different ones between them ("A X B" - "A Y B").
        if last_start1 < start1 && last_start2 < start2 {
            self.match_range(last_start1, last_start2, last_end1, last_end2)?;
            return self.match_range(start1, start2, end1, end2);
        }
        // On one side they are consecutive and on the other they are not ("A B" - "A Y B").
        self.match_complex_range(
            [last_start1, last_start2, last_end1, last_end2],
            [start1, start2, end1, end2],
        )
    }

    fn match_range(
        &mut self,
        start1: usize,
        start2: usize,
        end1: usize,
        end2: usize,
    ) -> Checked<()> {
        if start1 == end1 && start2 == end2 {
            return Ok(());
        }
        let sequence1 = &self.words1.text[start1..end1];
        let sequence2 = &self.words2.text[start2..end2];
        for ch in compare_punctuation(sequence1, sequence2, self.deadline)?.unchanged {
            self.builder.mark_equal(
                start1 + ch.start1,
                start2 + ch.start2,
                start1 + ch.end1,
                start2 + ch.end2,
            );
        }
        Ok(())
    }

    fn match_complex_range(
        &mut self,
        [start11, start12, end11, end12]: [usize; 4],
        [start21, start22, end21, end22]: [usize; 4],
    ) -> Checked<()> {
        if start11 == start21 && end11 == end21 {
            self.match_complex_range_left(start11, end11, start12, end12, start22, end22)
        } else if start12 == start22 && end12 == end22 {
            self.match_complex_range_right(start12, end12, start11, end11, start21, end21)
        } else {
            // IntelliJ throws `IllegalStateException`: it is not reached with really equal ranges.
            Ok(())
        }
    }

    fn match_complex_range_left(
        &mut self,
        start1: usize,
        end1: usize,
        start12: usize,
        end12: usize,
        start22: usize,
        end22: usize,
    ) -> Checked<()> {
        let sequence1 = &self.words1.text[start1..end1];
        let sequence21 = &self.words2.text[start12..end12];
        let sequence22 = &self.words2.text[start22..end22];

        let (first, second) =
            compare_punctuation_2side(sequence1, sequence21, sequence22, self.deadline)?;

        for ch in first.unchanged {
            self.builder.mark_equal(
                start1 + ch.start1,
                start12 + ch.start2,
                start1 + ch.end1,
                start12 + ch.end2,
            );
        }
        for ch in second.unchanged {
            self.builder.mark_equal(
                start1 + ch.start1,
                start22 + ch.start2,
                start1 + ch.end1,
                start22 + ch.end2,
            );
        }
        Ok(())
    }

    fn match_complex_range_right(
        &mut self,
        start2: usize,
        end2: usize,
        start11: usize,
        end11: usize,
        start21: usize,
        end21: usize,
    ) -> Checked<()> {
        let sequence11 = &self.words1.text[start11..end11];
        let sequence12 = &self.words1.text[start21..end21];
        let sequence2 = &self.words2.text[start2..end2];

        let (first, second) =
            compare_punctuation_2side(sequence2, sequence11, sequence12, self.deadline)?;

        // With the sides swapped: `compare2Side` has two texts on the right and here they are on
        // the left.
        for ch in first.unchanged {
            self.builder.mark_equal(
                start11 + ch.start2,
                start2 + ch.start1,
                start11 + ch.end2,
                start2 + ch.end1,
            );
        }
        for ch in second.unchanged {
            self.builder.mark_equal(
                start21 + ch.start2,
                start2 + ch.start1,
                start21 + ch.end2,
                start2 + ch.end1,
            );
        }
        Ok(())
    }
}

// `ByWordRt.comparePunctuation2Side`: `text1` against `text21` and `text22` as if they were one.
fn compare_punctuation_2side(
    text1: &[u8],
    text21: &[u8],
    text22: &[u8],
    deadline: Instant,
) -> Checked<(Diff, Diff)> {
    let text2 = [text21, text22].concat();
    let changes = compare_punctuation(text1, &text2, deadline)?;
    let offset = text21.len();

    // `splitIterable2Side`.
    let mut ranges1 = Vec::new();
    let mut ranges2 = Vec::new();
    for ch in changes.unchanged {
        if ch.end2 <= offset {
            ranges1.push(ch);
        } else if ch.start2 >= offset {
            ranges2.push(Range::new(
                ch.start1,
                ch.end1,
                ch.start2 - offset,
                ch.end2 - offset,
            ));
        } else {
            let len2 = offset - ch.start2;
            ranges1.push(Range::new(ch.start1, ch.start1 + len2, ch.start2, offset));
            ranges2.push(Range::new(ch.start1 + len2, ch.end1, 0, ch.end2 - offset));
        }
    }

    Ok((
        Diff::from_unchanged(ranges1, text1.len(), text21.len()),
        Diff::from_unchanged(ranges2, text1.len(), text22.len()),
    ))
}

// `ByCharRt.comparePunctuation`: only the punctuation marks; the rest is left unmatched.
fn compare_punctuation(text1: &[u8], text2: &[u8], deadline: Instant) -> Checked<Diff> {
    // `getPunctuationChars`: the marks are ASCII, one byte.
    let punctuation = |text: &[u8]| -> (Vec<u8>, Vec<usize>) {
        text.iter()
            .enumerate()
            .filter(|(_, c)| is_punctuation(u32::from(**c)))
            .map(|(offset, c)| (*c, offset))
            .unzip()
    };
    let (chars1, offsets1) = punctuation(text1);
    let (chars2, offsets2) = punctuation(text2);

    // `transferPunctuation`.
    let mut builder = EqualBuilder::new(text1.len(), text2.len());
    for range in diff_checked(&chars1, &chars2, deadline)?.unchanged {
        for i in 0..range.end1 - range.start1 {
            builder.mark_equal_one(offsets1[range.start1 + i], offsets2[range.start2 + i]);
        }
    }
    Ok(builder.finish())
}

// `ByWordRt.DefaultCorrector`: equal spaces at the edges of a change do not change.
fn default_corrector(texts: [&[u8]; 2], iterable: &Diff) -> Vec<Range> {
    iterable
        .changes()
        .into_iter()
        .filter_map(|range| {
            let starts = [range.start1, range.start2];
            let end_cut = expand_whitespaces_backward(texts, starts, [range.end1, range.end2]);
            let ends = [range.end1 - end_cut, range.end2 - end_cut];
            let start_cut = expand_whitespaces_forward(texts, starts, ends);
            let expand = Range::new(
                range.start1 + start_cut,
                ends[0],
                range.start2 + start_cut,
                ends[1],
            );
            (!expand.is_empty()).then_some(expand)
        })
        .collect()
}

// `ByWordRt.MergeDefaultCorrector`.
fn merge_default_corrector(texts: [&[u8]; 3], conflicts: &[MergeRange]) -> Vec<MergeRange> {
    conflicts
        .iter()
        .filter_map(|range| {
            let starts = [range.start1, range.start2, range.start3];
            let ends = [range.end1, range.end2, range.end3];
            let end_cut = expand_whitespaces_backward3(texts, starts, ends);
            let ends = ends.map(|end| end - end_cut);
            let start_cut = expand_whitespaces_forward3(texts, starts, ends);
            let expand = merge_range(starts.map(|start| start + start_cut), ends);
            (!expand.is_empty()).then_some(expand)
        })
        .collect()
}

// `ByWordRt.IgnoreSpacesCorrector`: matches the spaces if it can and, if not, ignores them.
fn ignore_spaces_corrector(texts: [&[u8]; 2], iterable: &Diff) -> Vec<Range> {
    iterable
        .changes()
        .into_iter()
        .filter_map(|range| {
            let trimmed = trim2(texts, expand_whitespaces(texts, range));
            (!trimmed.is_empty() && !is_equals_ignore_whitespaces2(texts, trimmed))
                .then_some(trimmed)
        })
        .collect()
}

// `ByWordRt.MergeIgnoreSpacesCorrector`.
fn merge_ignore_spaces_corrector(texts: [&[u8]; 3], conflicts: &[MergeRange]) -> Vec<MergeRange> {
    conflicts
        .iter()
        .filter_map(|range| {
            let trimmed = trim3(texts, expand_whitespaces3(texts, *range));
            (!trimmed.is_empty() && !is_equals_ignore_whitespaces3(texts, trimmed))
                .then_some(trimmed)
        })
        .collect()
}

//
// MergeResolveUtil
//

// `MergeResolveUtil.tryResolve`: first exact and, if not, without spaces. A comparison
// that is too big (`DiffTooBigException`) or that exceeds `RESOLVE_DEADLINE` is left unresolved.
pub fn try_resolve(left: &str, base: &str, right: &str) -> Option<String> {
    try_resolve_until(left, base, right, Instant::now() + RESOLVE_DEADLINE)
}

fn try_resolve_until(left: &str, base: &str, right: &str, deadline: Instant) -> Option<String> {
    let resolve = || -> Checked<Option<String>> {
        let resolved =
            SimpleHelper::new(left, base, right).execute(Whitespace::Default, deadline)?;
        if resolved.is_some() {
            return Ok(resolved);
        }
        SimpleHelper::new(left, base, right).execute(Whitespace::Ignore, deadline)
    };
    resolve().ok().flatten()
}

// `MergeResolveUtil.tryGreedyResolve`. It is taken for granted that the user reviews what was resolved and can
// undo it, so more risk is taken:
// - two different insertions in the same place are not resolved, because there is no good order;
// - a deletion and an insertion are both applied;
// - two deletions are joined;
// - a modification is a deletion plus an insertion.
fn try_greedy_resolve_until(
    left: &str,
    base: &str,
    right: &str,
    deadline: Instant,
) -> Option<String> {
    let resolve = || -> Checked<Option<String>> {
        let resolved =
            GreedyHelper::new(left, base, right).execute(Whitespace::Default, deadline)?;
        if resolved.is_some() {
            return Ok(resolved);
        }
        GreedyHelper::new(left, base, right).execute(Whitespace::Ignore, deadline)
    };
    resolve().ok().flatten()
}

// `MergeResolveUtil.SimpleHelper`: outside the changes, whatever one side changed; in each
// change by words, the side that changes, or nothing if it is a conflict.
struct SimpleHelper<'a> {
    texts: [&'a str; 3],
    new_content: Vec<u8>,
    last: [usize; 3],
}

impl<'a> SimpleHelper<'a> {
    fn new(left: &'a str, base: &'a str, right: &'a str) -> Self {
        Self {
            texts: [left, base, right],
            new_content: Vec::new(),
            last: [0; 3],
        }
    }

    fn execute(mut self, policy: Whitespace, deadline: Instant) -> Checked<Option<String>> {
        let [left, base, right] = self.texts;
        let changes = compare_words3(self.texts, policy, deadline)?;

        for fragment in changes {
            let base_range =
                self.next_merge_range([fragment.start1, fragment.start2, fragment.start3]);
            self.append_base(base_range);

            let conflict_range =
                self.next_merge_range([fragment.end1, fragment.end2, fragment.end3]);
            if !self.append_conflict(conflict_range, policy) {
                return Ok(None);
            }
        }

        let trailing_range = self.next_merge_range([left.len(), base.len(), right.len()]);
        self.append_base(trailing_range);

        Ok(String::from_utf8(self.new_content).ok())
    }

    fn next_merge_range(&mut self, ends: [usize; 3]) -> MergeRange {
        let range = merge_range(self.last, ends);
        self.last = ends;
        range
    }

    fn append_base(&mut self, range: MergeRange) {
        if range.is_empty() {
            return;
        }
        let policy = Whitespace::Default;
        if self.is_unchanged_range(&range, policy) {
            self.append(&range, ThreeSide::Base);
            return;
        }
        let found = self.get_conflict_type(&range, policy);
        if found.left_change {
            self.append(&range, ThreeSide::Left);
        } else if found.right_change {
            self.append(&range, ThreeSide::Right);
        } else {
            self.append(&range, ThreeSide::Base);
        }
    }

    fn append_conflict(&mut self, range: MergeRange, policy: Whitespace) -> bool {
        let found = self.get_conflict_type(&range, policy);
        if found.kind == BlockKind::Conflict {
            return false;
        }
        let side = if found.left_change {
            ThreeSide::Left
        } else {
            ThreeSide::Right
        };
        self.append(&range, side);
        true
    }

    fn append(&mut self, range: &MergeRange, side: ThreeSide) {
        let text = side.select(self.texts).as_bytes();
        self.new_content
            .extend_from_slice(&text[range.start(side)..range.end(side)]);
    }

    fn content(&self, range: &MergeRange, side: ThreeSide) -> &'a [u8] {
        &side.select(self.texts).as_bytes()[range.start(side)..range.end(side)]
    }

    // `MergeRangeUtil.getWordMergeType`.
    fn get_conflict_type(&self, range: &MergeRange, policy: Whitespace) -> MergeConflictType {
        get_merge_type(
            |side| range.start(side) == range.end(side),
            |side1, side2| self.compare_contents(range, policy, side1, side2),
            None,
            || false,
        )
    }

    // `MergeRangeUtil.compareWordMergeContents`.
    fn compare_contents(
        &self,
        range: &MergeRange,
        policy: Whitespace,
        side1: ThreeSide,
        side2: ThreeSide,
    ) -> bool {
        is_equal_texts(
            self.content(range, side1),
            self.content(range, side2),
            policy,
        )
    }

    fn is_unchanged_range(&self, range: &MergeRange, policy: Whitespace) -> bool {
        self.compare_contents(range, policy, ThreeSide::Base, ThreeSide::Left)
            && self.compare_contents(range, policy, ThreeSide::Base, ThreeSide::Right)
    }
}

// `MergeResolveUtil.GreedyHelper`.
struct GreedyHelper<'a> {
    left: &'a str,
    base: &'a str,
    right: &'a str,
    new_content: Vec<u8>,
    last_base_offset: usize,
}

impl<'a> GreedyHelper<'a> {
    fn new(left: &'a str, base: &'a str, right: &'a str) -> Self {
        Self {
            left,
            base,
            right,
            new_content: Vec::new(),
            last_base_offset: 0,
        }
    }

    fn execute(mut self, policy: Whitespace, deadline: Instant) -> Checked<Option<String>> {
        let fragments1 = compare_words(self.base, self.left, policy, deadline)?;
        let fragments2 = compare_words(self.base, self.right, policy, deadline)?;
        let (mut index1, mut index2) = (0, 0);

        loop {
            let change_start1 = fragments1.get(index1).map(|fragment| fragment.start1);
            let change_start2 = fragments2.get(index2).map(|fragment| fragment.start1);

            // Up to the next group of changes.
            match (change_start1, change_start2) {
                (None, None) => {
                    self.append_base(self.base.len());
                    break;
                }
                (Some(start1), Some(start2)) => self.append_base(start1.min(start2)),
                (Some(start), None) | (None, Some(start)) => self.append_base(start),
            }

            // The group of changes that touch one another.
            let mut base_offset_end = self.last_base_offset;
            let (mut end1, mut end2) = (index1, index2);
            loop {
                if let Some(next1) = fragments1.get(end1) {
                    if next1.start1 <= base_offset_end {
                        base_offset_end = base_offset_end.max(next1.end1);
                        end1 += 1;
                        continue;
                    }
                }
                if let Some(next2) = fragments2.get(end2) {
                    if next2.start1 <= base_offset_end {
                        base_offset_end = base_offset_end.max(next2.end1);
                        end2 += 1;
                        continue;
                    }
                }
                break;
            }
            debug_assert!(index1 != end1 || index2 != end2);

            let inserted1 = inserted_content(self.left, &fragments1[index1..end1]);
            let inserted2 = inserted_content(self.right, &fragments2[index2..end2]);
            index1 = end1;
            index2 = end2;

            // Deletions are joined and applied.
            self.last_base_offset = base_offset_end;

            // Insertions without conflict are applied.
            if inserted1.is_empty() && inserted2.is_empty() {
                continue;
            }
            if inserted2.is_empty() {
                self.new_content.extend_from_slice(&inserted1);
                continue;
            }
            if inserted1.is_empty() {
                self.new_content.extend_from_slice(&inserted2);
                continue;
            }
            if is_equal_texts(&inserted1, &inserted2, policy) {
                let inserted = if inserted1.len() <= inserted2.len() {
                    inserted1
                } else {
                    inserted2
                };
                self.new_content.extend_from_slice(&inserted);
                continue;
            }

            // Two different insertions: not resolved.
            return Ok(None);
        }

        Ok(String::from_utf8(self.new_content).ok())
    }

    fn append_base(&mut self, end_offset: usize) {
        if end_offset <= self.last_base_offset {
            return;
        }
        self.new_content
            .extend_from_slice(&self.base.as_bytes()[self.last_base_offset..end_offset]);
        self.last_base_offset = end_offset;
    }
}

// `GreedyHelper.getInsertedContent`: what those changes put on their side.
fn inserted_content(text: &str, fragments: &[Range]) -> Vec<u8> {
    let bytes = text.as_bytes();
    fragments
        .iter()
        .flat_map(|fragment| &bytes[fragment.start2..fragment.end2])
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    // `ComparisonMergeUtilTestBase`: `lines_diff` is `compareLines` (ignored changes are
    // lost) and `lines_merge` is `mergeLines` (they stay apart, outside the conflicts).
    #[derive(Clone, Copy)]
    enum TestType {
        LineDiff,
        LineMerge,
    }

    // Deadlines that do not expire even if the machine is loaded: the tests do not depend on the clock.
    const FAR: Duration = Duration::from_secs(60);
    const FAR_LIMITS: Limits = Limits {
        line_diff: FAR,
        resolve: FAR,
        budget: FAR,
    };

    fn far() -> Instant {
        Instant::now() + FAR
    }

    fn expired() -> Instant {
        Instant::now() - Duration::from_millis(1)
    }

    // `blocks` with far-off deadlines.
    fn blocks_far(left: &str, base: &str, right: &str, policy: Whitespace) -> Vec<Block> {
        blocks_within(left, base, right, policy, FAR_LIMITS)
    }

    // `ComparisonManagerImpl.compareLines` and `mergeLines` with three texts.
    fn compare_lines(
        left: &str,
        base: &str,
        right: &str,
        policy: Whitespace,
        keep_ignored_changes: bool,
    ) -> Vec<MergeRange> {
        let (left, base, right) = (split_lines(left), split_lines(base), split_lines(right));
        compare_line_ranges(&left, &base, &right, policy, keep_ignored_changes, FAR)
    }

    // `DiffTestCase.parseSource`.
    fn parse_source(text: &str) -> String {
        text.replace('_', "\n")
    }

    // `DiffTestCase.parseLineMatching`: a line with something other than spaces is changed.
    fn parse_line_matching(matching: &str, text: &str) -> BTreeSet<usize> {
        assert_eq!(matching.len(), text.len());
        let lines: Vec<&str> = matching.split(['_', '*']).collect();
        assert_eq!(lines.len(), text.split('\n').count());
        lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.chars().any(|c| c != ' '))
            .map(|(number, _)| number)
            .collect()
    }

    // `MergeTestBuilder.mod`.
    fn change(
        line1: usize,
        line2: usize,
        line3: usize,
        count1: usize,
        count2: usize,
        count3: usize,
    ) -> MergeRange {
        MergeRange {
            start1: line1,
            end1: line1 + count1,
            start2: line2,
            end2: line2 + count2,
            start3: line3,
            end3: line3 + count3,
        }
    }

    // `MergeTestBuilder.test`: the texts with `_` for a newline, and what is expected: which
    // lines of each side fall in some change, the list of changes, or both.
    fn check(
        kind: TestType,
        texts: [&str; 3],
        matching: Option<[&str; 3]>,
        changes: Option<&[MergeRange]>,
        policy: Whitespace,
    ) {
        assert!(changes.is_some() || matching.is_some());
        let [left, base, right] = texts.map(parse_source);
        let keep = matches!(kind, TestType::LineMerge);
        let actual = compare_lines(&left, &base, &right, policy, keep);

        if let Some(matching) = matching {
            let expected: Vec<BTreeSet<usize>> = matching
                .iter()
                .zip([&left, &base, &right])
                .map(|(matching, text)| parse_line_matching(matching, text))
                .collect();
            let mut sets = [BTreeSet::new(), BTreeSet::new(), BTreeSet::new()];
            for range in &actual {
                sets[0].extend(range.start1..range.end1);
                sets[1].extend(range.start2..range.end2);
                sets[2].extend(range.start3..range.end3);
            }
            assert_eq!(sets[0], expected[0], "Left: {actual:?}");
            assert_eq!(sets[1], expected[1], "Base: {actual:?}");
            assert_eq!(sets[2], expected[2], "Right: {actual:?}");
        }
        if let Some(changes) = changes {
            assert_eq!(actual, changes);
        }
    }

    fn lines_merge(
        texts: [&str; 3],
        matching: Option<[&str; 3]>,
        changes: Option<&[MergeRange]>,
        policy: Whitespace,
    ) {
        check(TestType::LineMerge, texts, matching, changes, policy);
    }

    fn lines_diff(texts: [&str; 3], changes: &[MergeRange], policy: Whitespace) {
        check(TestType::LineDiff, texts, None, Some(changes), policy);
    }

    // `LineComparisonMergeUtilTest`

    #[test]
    fn test_simple_cases() {
        use Whitespace::Default;
        lines_merge(["", "", ""], Some(["", "", ""]), Some(&[]), Default);
        lines_merge(
            ["x_y_z", "x_y_z", "x_y_z"],
            Some([" _ _ ", " _ _ ", " _ _ "]),
            Some(&[]),
            Default,
        );
        lines_merge(
            ["", "", "x"],
            None,
            Some(&[change(0, 0, 0, 1, 1, 1)]),
            Default,
        );
        lines_merge(
            ["", "", "_x"],
            Some(["", "", "_-"]),
            Some(&[change(1, 1, 1, 0, 0, 1)]),
            Default,
        );
        lines_merge(["x", "y", "x"], Some(["-", "-", "-"]), None, Default);
        lines_merge(["x", "x_y", "x"], Some([" ", " _-", " "]), None, Default);
        lines_merge(["x", "y_x", "x"], Some([" ", "-_ ", " "]), None, Default);
        lines_merge(
            ["x_y_z", "x_Y_z", "x_y_z"],
            Some([" _-_ ", " _-_ ", " _-_ "]),
            None,
            Default,
        );
        lines_merge(
            ["x_y_z", "X_y_Z", "x_y_z"],
            Some(["-_ _-", "-_ _-", "-_ _-"]),
            None,
            Default,
        );
    }

    #[test]
    fn test_ignore_whitespaces() {
        use Whitespace::Ignore;
        lines_diff(["x_ y _z", "x_y_z", "x_  y_  z"], &[], Ignore);
        lines_merge(
            ["x_ y _z", "x_y_z", "x_  y_  z"],
            None,
            Some(&[change(1, 1, 1, 2, 2, 2)]),
            Ignore,
        );

        lines_diff(
            ["x_ y _z", "x_Y_z", "x_  y_  z"],
            &[change(1, 1, 1, 1, 1, 1)],
            Ignore,
        );
        lines_merge(
            ["x_ y _z", "x_Y_z", "x_  y_  z"],
            None,
            Some(&[change(1, 1, 1, 1, 1, 1), change(2, 2, 2, 1, 1, 1)]),
            Ignore,
        );

        lines_merge(
            ["x_ y _z", "x_Y_z", "x_  y_z"],
            None,
            Some(&[change(1, 1, 1, 1, 1, 1)]),
            Ignore,
        );
        lines_merge(
            ["x_ y _z", "x_y_z", "x_  y_z"],
            None,
            Some(&[change(1, 1, 1, 1, 1, 1)]),
            Ignore,
        );
        lines_merge(
            [" x _y_z", "x_y_z", "x_y_  z"],
            None,
            Some(&[change(0, 0, 0, 1, 1, 1), change(2, 2, 2, 1, 1, 1)]),
            Ignore,
        );

        lines_diff(
            [" x_new_y_z", "x_ y _z", "x_y_new_ z"],
            &[change(1, 1, 1, 1, 0, 0), change(3, 2, 2, 0, 0, 1)],
            Ignore,
        );
        lines_merge(
            [" x_new_y_z", "x_ y _z", "x_y_new_ z"],
            None,
            Some(&[
                change(0, 0, 0, 1, 1, 1),
                change(1, 1, 1, 1, 0, 0),
                change(2, 1, 1, 1, 1, 1),
                change(3, 2, 2, 0, 0, 1),
                change(3, 2, 3, 1, 1, 1),
            ]),
            Ignore,
        );

        lines_diff(
            [" x_new_y1_z", "x_ y _z", "x_y2_new_ z"],
            &[change(1, 1, 1, 2, 1, 2)],
            Ignore,
        );
        lines_merge(
            [" x_new_y1_z", "x_ y _z", "x_y2_new_ z"],
            None,
            Some(&[
                change(0, 0, 0, 1, 1, 1),
                change(1, 1, 1, 2, 1, 2),
                change(3, 2, 3, 1, 1, 1),
            ]),
            Ignore,
        );
    }

    // `MergeResolveUtilTest`: the arguments go in their order, base, left and right.

    fn check_simple(base: &str, left: &str, right: &str, expected: Option<&str>) {
        assert_eq!(
            try_resolve_until(left, base, right, far()).as_deref(),
            expected
        );
    }

    fn check_greedy(base: &str, left: &str, right: &str, expected: Option<&str>) {
        assert_eq!(
            try_greedy_resolve_until(left, base, right, far()).as_deref(),
            expected
        );
    }

    fn check_both(base: &str, left: &str, right: &str, expected: Option<&str>) {
        check_simple(base, left, right, expected);
        check_greedy(base, left, right, expected);
    }

    #[test]
    fn test_simple() {
        check_both("", "", "", Some(""));
        check_both("x x x", "x x x", "x x x", Some("x x x"));
        check_both("x x x", "x Y x", "x x x", Some("x Y x"));
        check_both("x x", "x x", "x Y x", Some("x Y x"));
        check_both("x X x", "x x", "x X x", Some("x x"));
    }

    #[test]
    fn test_same_modification() {
        check_both("x x x", "x Y x", "x Y x", Some("x Y x"));
        check_both("x x", "x Y x", "x Y x", Some("x Y x"));
        check_both("x X x", "x x", "x x", Some("x x"));
    }

    #[test]
    fn test_non_conflicting_changes() {
        check_both("x x x", "x Y x x", "x x Z x", Some("x Y x Z x"));
        check_both("x", "x Y", "Z x", Some("Z x Y"));
        check_both("x x", "x", "Z x x", Some("Z x"));
    }

    #[test]
    fn test_failure() {
        check_both("x x x", "x Y x", "x Z x", None);
        check_both("x x", "x Y x", "x Z x", None);
    }

    #[test]
    fn test_non_failure_conflicts() {
        check_greedy("x X x", "x x", "x X Y x", Some("x Y x"));
        check_greedy("x X x", "x x", "x Y X x", Some("x Y x"));
        check_greedy("x X Y x", "x X x", "x Y x", Some("x x"));
        check_greedy("x X Y Z x", "x X x", "x Z x", Some("x x"));
        check_greedy(
            "x A B C D E F G H K x",
            "x C F K x",
            "x A D H x",
            Some("x x"),
        );
    }

    #[test]
    fn test_confusing_conflicts() {
        // These cases could also fail.
        check_greedy("x X x", "x x", "x Z x", Some("xZ x"));
        check_greedy("x X X x", "x X Y X x", "x x", Some("x Y x"));
        check_greedy("x X x", "x x", "x Y x", Some("xY x"));
        check_greedy("x X X x", "x Y x", "x X Y x", Some("x Y x"));
        // IDEA-200696
        check_both(
            "version: 1.0.0",
            "version: 2.0.0",
            "version: 1.0.4",
            Some("version: 2.0.4"),
        );
    }

    // The first case of `testRegressions`. The words of the base, `i` and the newline, against those of
    // the right, the newline and `i`, have two alignments of the same length. Myers matches the
    // `i`: the right inserts a newline in front and both sides delete the one at the end, and "\ni" comes out
    // with both variants. The "fair" diff matches the newline: everything is a conflict, which the pass
    // without spaces leaves as "i".
    #[test]
    #[ignore = "decision 4: Myers matches the `i` of \"i\\n\" with the one of \"\\ni\" and gives \"\\ni\"; \
                IntelliJ matches the newline and expects \"i\""]
    fn test_regressions_initial_newline() {
        check_simple("i\n", "i", "\ni", Some("i"));
        check_greedy("i\n", "i", "\ni", Some("i"));
    }

    // What Myers gives in that case, so that a change in `similar` does not go unseen.
    #[test]
    fn with_myers_the_initial_newline_is_kept() {
        check_simple("i\n", "i", "\ni", Some("\ni"));
        check_greedy("i\n", "i", "\ni", Some("\ni"));
    }

    // The third case of `testRegressions`, in the order of `test`: base, left, right and what
    // is expected.
    const CHANGE_KIND_BASE: &str = r#"
  public static class ChangeData {
    @NotNull public final ChangeKind kind;
    public final int otherPath;

    public ChangeData(@NotNull ChangeKind kind, int otherPath) {
      this.kind = kind;
      this.otherPath = otherPath;
    }

    public boolean isRename() {
      return kind.equals(ChangeKind.RENAMED_FROM) || kind.equals(ChangeKind.RENAMED_TO);
    }

    @Override
    public int hashCode() {
      return Objects.hash(kind, otherPath);
    }
  }

  private enum ChangeKind {
    MODIFIED((byte)0),
    RENAMED_FROM((byte)1),
    RENAMED_TO((byte)2);"#;

    const CHANGE_KIND_LEFT: &str = r#"
  public enum ChangeKind {
    NOT_CHANGED((byte)-1),
    MODIFIED((byte)0),
    ADDED((byte)1),
    REMOVED((byte)2);"#;

    const CHANGE_KIND_RIGHT: &str = r#"
  public static class ChangeData {
  @NotNull public final ChangeKind kind;
  public final int otherPath;

  public ChangeData(@NotNull ChangeKind kind, int otherPath) {
    this.kind = kind;
    this.otherPath = otherPath;
  }

  public boolean isRename() {
    return kind.equals(ChangeKind.RENAMED_FROM) || kind.equals(ChangeKind.RENAMED_TO);
  }

  @Override
  public int hashCode() {
    return Objects.hash(kind, otherPath);
  }
}

private enum ChangeKind {
  MODIFIED((byte)0),
  RENAMED_FROM((byte)1),
  RENAMED_TO((byte)2);"#;

    const CHANGE_KIND_RESOLVED: &str = r#"
  public enum ChangeKind {
    NOT_CHANGED((byte)-1),
  MODIFIED((byte)0),
  ADDED((byte)1),
  REMOVED((byte)2);"#;

    // `testRegressions` without its first case, which goes apart, and with the third only in `testSimple`.
    #[test]
    fn test_regressions() {
        check_simple("Y X Y", "Y C\nX\nC Y", "Y \nX\n Y", Some("Y \nC\nX\nC Y"));

        check_simple(
            CHANGE_KIND_BASE,
            CHANGE_KIND_LEFT,
            CHANGE_KIND_RIGHT,
            Some(CHANGE_KIND_RESOLVED),
        );
    }

    // The `testGreedy` half of the third case of `testRegressions`. The left keeps only the enum and
    // the right reindents. From the base to the left there are two alignments of words of the same
    // length: Myers matches the `ChangeKind` and the newline of `enum ChangeKind {` with those of the field
    // `@NotNull public final ChangeKind kind;`, and the "fair" diff with those of the enum. With Myers,
    // `NOT_CHANGED` keeps the two-space indentation of the right instead of the four of the
    // left; the rest of the text matches. Once the tie is removed (renaming the `ChangeKind` of the
    // class), what IntelliJ expects comes out.
    #[test]
    #[ignore = "decision 4: Myers aligns `ChangeKind {` with the class field and not with the enum; \
                the greedy variant gives `  NOT_CHANGED((byte)-1),` where IntelliJ expects \
                `    NOT_CHANGED((byte)-1),`"]
    fn test_regressions_change_kind_greedy() {
        check_greedy(
            CHANGE_KIND_BASE,
            CHANGE_KIND_LEFT,
            CHANGE_KIND_RIGHT,
            Some(CHANGE_KIND_RESOLVED),
        );
    }

    // What Myers gives in that case, so that a change in `similar` does not go unseen.
    #[test]
    fn with_myers_not_changed_keeps_the_right_indentation() {
        let expected = CHANGE_KIND_RESOLVED.replace("    NOT_CHANGED", "  NOT_CHANGED");
        check_greedy(
            CHANGE_KIND_BASE,
            CHANGE_KIND_LEFT,
            CHANGE_KIND_RIGHT,
            Some(&expected),
        );
    }

    // Branch-specific.

    fn block(
        left: (u32, u32),
        base: (u32, u32),
        right: (u32, u32),
        kind: BlockKind,
        changed: (bool, bool),
        auto: Option<&str>,
    ) -> Block {
        Block {
            left,
            base,
            right,
            kind,
            left_changed: changed.0,
            right_changed: changed.1,
            auto: auto.map(str::to_string),
        }
    }

    #[test]
    fn block_without_conflict_on_each_side() {
        let found = blocks_far(
            "a\nB\nc\nd\ne\n",
            "a\nb\nc\nd\ne\n",
            "a\nb\nc\nD\ne\n",
            Whitespace::Default,
        );

        assert_eq!(
            found,
            [
                block(
                    (1, 2),
                    (1, 2),
                    (1, 2),
                    BlockKind::Modified,
                    (true, false),
                    None
                ),
                block(
                    (3, 4),
                    (3, 4),
                    (3, 4),
                    BlockKind::Modified,
                    (false, true),
                    None
                ),
            ]
        );
    }

    #[test]
    fn same_change_on_both_sides_is_not_a_conflict() {
        let found = blocks_far("a\nX\nc\n", "a\nb\nc\n", "a\nX\nc\n", Whitespace::Default);

        assert_eq!(
            found,
            [block(
                (1, 2),
                (1, 2),
                (1, 2),
                BlockKind::Modified,
                (true, true),
                None
            )]
        );
    }

    #[test]
    fn deletion_on_one_side_and_change_on_the_other_is_a_conflict_without_auto() {
        let found = blocks_far("a\nc\n", "a\nb\nc\n", "a\nB\nc\n", Whitespace::Default);

        assert_eq!(
            found,
            [block(
                (1, 1),
                (1, 2),
                (1, 2),
                BlockKind::Conflict,
                (true, true),
                None
            )]
        );
    }

    // A conflict that resolves itself carries the text of the resolution.
    #[test]
    fn conflict_that_resolves_itself_carries_auto() {
        let found = blocks_far(
            "a\nx Y x x\nc\n",
            "a\nx x x\nc\n",
            "a\nx x Z x\nc\n",
            Whitespace::Default,
        );

        assert_eq!(
            found,
            [block(
                (1, 2),
                (1, 2),
                (1, 2),
                BlockKind::Conflict,
                (true, true),
                Some("x Y x Z x"),
            )]
        );
    }

    // With `Ignore`, the lines that only change in spaces, on one side and the other, end up in a
    // single block, which is no longer a conflict: `IgnoringChangeBuilder` joins the consecutive ignored ones.
    #[test]
    fn ignoring_whitespace_merges_blocks() {
        let (left, base, right) = ("a\nb \nc\nd\n", "a\nb\nc\nd\n", "a\nb\nc \nd\n");

        let strict = blocks_far(left, base, right, Whitespace::Default);
        let ignoring = blocks_far(left, base, right, Whitespace::Ignore);

        assert_eq!(strict.len(), 1);
        assert_eq!(strict[0].kind, BlockKind::Conflict);
        assert_eq!(
            ignoring,
            [block(
                (1, 3),
                (1, 3),
                (1, 3),
                BlockKind::Modified,
                (true, true),
                None
            )]
        );
    }

    // Words from a short vocabulary, so that almost all of them appear on both sides and Myers cannot
    // discard them.
    fn random_words(seed: u64, bytes: usize, separator: &str) -> String {
        const WORDS: [&str; 8] = [
            "alef", "beta", "game", "delta", "eps", "zeta", "eta", "theta",
        ];
        let mut state = seed;
        let mut text = String::with_capacity(bytes + 8);
        while text.len() < bytes {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            text.push_str(WORDS[(state % 8) as usize]);
            text.push_str(separator);
        }
        text
    }

    // Before the diff: after removing what is common at the ends and what is not on the other side
    // there remain 30,000 `a` against one, and the differences are at least 29,999, more than
    // 20000 + 10·√30001.
    #[test]
    fn exceeding_intellijs_limit_is_too_big() {
        let many = vec!["a"; 30000];
        let few = vec!["b", "a", "b"];

        assert_eq!(diff_checked(&many, &few, far()).err(), Some(TooBig));
        assert!(diff_checked(&many[..100], &few, far()).is_ok());
        // With nothing in common after `discardUnique`, IntelliJ gives a single change and does not fail.
        assert!(diff_checked(&many, &["a", "b"], far()).is_ok());
    }

    // After the diff: both sides have 30,000 shared elements, so the prior bound
    // is not enough, but Myers only matches 15,000 and 30,000 differences remain, more than
    // 20000 + 10·√60000.
    #[test]
    fn exceeding_the_limit_after_the_diff_is_too_big() {
        let half = |first: &'static str, second: &'static str| {
            let mut data = vec![first; 15000];
            data.extend(vec![second; 15000]);
            data
        };
        let (ab, ba) = (half("a", "b"), half("b", "a"));

        assert_eq!(diff_checked(&ab, &ba, far()).err(), Some(TooBig));
        assert!(diff_checked(&ab, &ab, far()).is_ok());
    }

    #[test]
    fn an_expired_deadline_leaves_it_unresolved() {
        let (left, base, right) = ("x Y x x", "x x x", "x x Z x");

        assert_eq!(
            try_resolve_until(left, base, right, far()).as_deref(),
            Some("x Y x Z x")
        );
        assert_eq!(try_resolve_until(left, base, right, expired()), None);
        assert_eq!(try_greedy_resolve_until(left, base, right, expired()), None);
    }

    #[test]
    fn diff_checked_with_an_expired_deadline_is_too_big() {
        let text = random_words(8, 60_000, " ");
        let base: Vec<&str> = text.split(' ').collect();
        let mut other = base.clone();
        other[5000] = "another";

        assert!(diff_checked(&base, &other, far()).is_ok());
        assert_eq!(diff_checked(&base, &other, expired()).err(), Some(TooBig));
    }

    // Line diffs do not fail when the deadline expires: the alignment they give is still valid.
    #[test]
    fn diff_with_an_expired_deadline_gives_a_valid_alignment() {
        let text1 = random_words(9, 200_000, "\n");
        let text2 = random_words(10, 200_000, "\n");
        let (lines1, lines2) = (split_lines(&text1), split_lines(&text2));

        let found = diff(&lines1, &lines2, expired());

        let (mut last1, mut last2) = (0, 0);
        for range in &found.unchanged {
            assert!(last1 <= range.start1 && last2 <= range.start2);
            assert_eq!(range.end1 - range.start1, range.end2 - range.start2);
            assert!(range.end1 <= lines1.len() && range.end2 <= lines2.len());
            assert_eq!(
                lines1[range.start1..range.end1],
                lines2[range.start2..range.end2]
            );
            (last1, last2) = (range.end1, range.end2);
        }
    }

    #[test]
    fn a_big_block_with_few_changes_is_resolved() {
        let base = random_words(7, 200_000, " ");
        let left = format!("LEFT {base}");
        let right = format!("{base} RIGHT");

        assert_eq!(
            try_resolve_until(&left, &base, &right, far()),
            Some(format!("LEFT {base} RIGHT"))
        );
    }

    // A line of 1 MiB: IntelliJ's limit trips before running Myers, without depending on the
    // deadline.
    #[test]
    fn resolving_a_huge_block_returns_none_without_hanging() {
        let base = "a ".repeat(1 << 19);
        let left = "b a b";
        let right = format!("{base}Z");

        assert_eq!(try_resolve_until(left, &base, &right, far()), None);
    }

    // Smoke test with the real deadlines: three very different 1 MiB files finish.
    // The limit is generous on purpose; the exact checks are the ones above.
    #[test]
    fn blocks_of_huge_files_finish_in_time() {
        let base = random_words(4, 1 << 20, "\n");
        let left = random_words(5, 1 << 20, "\n");
        let right = random_words(6, 1 << 20, "\n");

        let started = Instant::now();
        let found = blocks(&left, &base, &right, Whitespace::Default);

        assert!(!found.is_empty());
        assert!(started.elapsed() < Duration::from_secs(60));
    }

    #[test]
    fn the_block_is_serialized_for_the_interface() {
        let found = blocks_far("x\n", "y\n", "z\n", Whitespace::Default);

        let json = serde_json::to_value(&found).unwrap();

        assert_eq!(
            json,
            serde_json::json!([{
                "left": [0, 1],
                "base": [0, 1],
                "right": [0, 1],
                "kind": "conflict",
                "leftChanged": true,
                "rightChanged": true,
                "auto": null,
            }])
        );
        let policy: Whitespace = serde_json::from_str("\"ignore\"").unwrap();
        assert_eq!(policy, Whitespace::Ignore);
    }
}
