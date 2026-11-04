use std::collections::HashMap;

use crate::log::graph::{GraphEdge, LinearGraph};

const MAX_BLOCK_SIZE: usize = 20;
const MAX_DELTA_TIME: i64 = 60 * 60 * 24 * 3 * 1000;
const SMALL_DELTA_TIME: i64 = 60 * 60 * 4 * 1000;

// `BekEdgeRestrictions`: a row does not enter the order until its parents from another branch are
// in. Counting per row is enough: each time, what was put in is taken out, repetitions included.
struct EdgeRestrictions {
    pending: Vec<u32>,
    ups_of_down: HashMap<u32, Vec<u32>>,
}

impl EdgeRestrictions {
    fn new(size: usize) -> Self {
        Self {
            pending: vec![0; size],
            ups_of_down: HashMap::new(),
        }
    }

    fn add(&mut self, up: u32, down: u32) {
        self.pending[up as usize] += 1;
        self.ups_of_down.entry(down).or_default().push(up);
    }

    fn remove(&mut self, down: u32) {
        for up in self.ups_of_down.remove(&down).unwrap_or_default() {
            self.pending[up as usize] -= 1;
        }
    }

    fn has(&self, up: u32) -> bool {
        self.pending[up as usize] > 0
    }
}

// `BekBranch`: the commits of a chain and, at the end, the block that is next to insert. Rows are
// inserted from bottom to top, so the block comes from the end of what is left in `nodes`.
struct Branch {
    nodes: Vec<u32>,
    no_insert: usize,
    prepared: Option<usize>,
}

impl Branch {
    fn new(nodes: Vec<u32>) -> Self {
        let no_insert = nodes.len();
        Self {
            nodes,
            no_insert,
            prepared: None,
        }
    }

    fn is_done(&self) -> bool {
        self.no_insert == 0
    }

    fn prepare(&mut self, graph: &LinearGraph, times: &[i64], restrictions: &EdgeRestrictions) {
        if restrictions.has(self.nodes[self.no_insert - 1]) {
            return;
        }
        let mut previous = self.no_insert - 1;
        while previous > 0 {
            let up = self.nodes[previous - 1];
            let down = self.nodes[previous];
            if restrictions.has(up) {
                break;
            }
            let downs = graph.downs.of(up);
            let contains = downs.iter().any(|edge| edge.down == down);
            if downs.len() > 1 && contains {
                previous -= 1;
                continue;
            }
            if !contains {
                break;
            }
            let delta = (times[up as usize] - times[down as usize]).abs();
            if delta > MAX_DELTA_TIME {
                break;
            }
            if previous + MAX_BLOCK_SIZE < self.no_insert && delta > SMALL_DELTA_TIME {
                break;
            }
            previous -= 1;
        }
        self.prepared = Some(previous);
    }

    fn prepared_part(&self) -> Option<&[u32]> {
        self.prepared
            .map(|start| &self.nodes[start..self.no_insert])
    }

    fn timestamp(&self, times: &[i64]) -> i64 {
        self.prepared_part()
            .map_or(i64::MAX, |part| times[part[0] as usize])
    }

    fn done_insert(&mut self) {
        if let Some(start) = self.prepared.take() {
            self.no_insert = start;
        }
    }
}

// `BekBranchCreator.createNextBranch`: goes down from the head, and among the parents tries the
// last one first, so that what was merged ends up next to the merge.
fn create_branch(
    graph: &LinearGraph,
    layout: &[u32],
    head: u32,
    done: &mut [bool],
    restrictions: &mut EdgeRestrictions,
) -> Vec<u32> {
    let layout_of = |row: u32| layout[row as usize];
    let start_layout = layout_of(head);
    let mut nodes = vec![head];
    done[head as usize] = true;
    let mut stack = vec![head];
    while let Some(current) = stack.last().copied() {
        let current_layout = layout_of(current);
        let mut next = None;
        for edge in graph.downs.of(current).iter().rev() {
            let down = edge.down;
            if done[down as usize] {
                if layout_of(down) < start_layout {
                    restrictions.add(current, down);
                }
            } else if current_layout <= layout_of(down) {
                let has_undone_ups =
                    graph.ups.of(down).iter().any(|edge| {
                        !done[edge.up as usize] && layout_of(edge.up) <= layout_of(down)
                    });
                if !has_undone_ups {
                    next = Some(down);
                    break;
                }
            }
        }
        match next {
            Some(down) => {
                done[down as usize] = true;
                nodes.push(down);
                stack.push(down);
            }
            None => {
                stack.pop();
            }
        }
    }
    nodes
}

// `BekBranchMerger`: from bottom to top, the block with the oldest date always goes in.
fn merge_branches(
    graph: &LinearGraph,
    times: &[i64],
    mut branches: Vec<Branch>,
    mut restrictions: EdgeRestrictions,
) -> Vec<u32> {
    let mut inverse = Vec::with_capacity(graph.rows.len());
    loop {
        let mut has_undone = false;
        for branch in branches.iter_mut().filter(|branch| !branch.is_done()) {
            has_undone = true;
            if branch.prepared.is_none() {
                branch.prepare(graph, times, &restrictions);
            }
        }
        if !has_undone {
            break;
        }
        let mut selected = 0;
        for index in 0..branches.len() {
            if branches[selected].timestamp(times) > branches[index].timestamp(times) {
                selected = index;
            }
        }
        // It should not happen: IntelliJ would fail here. Git's order is always valid.
        let Some(part) = branches[selected].prepared_part() else {
            return (0..graph.len()).collect();
        };
        for node in part {
            restrictions.remove(*node);
        }
        inverse.extend(part.iter().rev());
        branches[selected].done_insert();
    }
    inverse.reverse();
    inverse
}

// `BekSorter`. `graph` is in the order of the store, and `layout` and `times` are per commit.
pub fn bek_order(graph: &LinearGraph, layout: &[u32], times: &[i64]) -> Vec<u32> {
    // The index of a head is the first of its own: sorting them by it recovers the order in which
    // they were traversed.
    let mut heads = graph.heads();
    heads.sort_unstable_by_key(|head| layout[*head as usize]);
    let mut done = vec![false; graph.rows.len()];
    let mut restrictions = EdgeRestrictions::new(graph.rows.len());
    let branches = heads
        .into_iter()
        .map(|head| {
            let nodes = create_branch(graph, layout, head, &mut done, &mut restrictions);
            Branch::new(nodes)
        })
        .collect();
    merge_branches(graph, times, branches, restrictions)
}

// The graph with the rows in `order`. Each commit keeps the order of its parents.
pub fn reordered(graph: &LinearGraph, order: &[u32]) -> LinearGraph {
    let mut position = vec![0; order.len()];
    for (row, usual) in order.iter().enumerate() {
        position[*usual as usize] = row as u32;
    }
    let rows = order.iter().map(|row| graph.rows[*row as usize]).collect();
    let edges: Vec<GraphEdge> = order
        .iter()
        .flat_map(|row| graph.downs.of(*row))
        .map(|edge| GraphEdge {
            up: position[edge.up as usize],
            down: position[edge.down as usize],
            style: edge.style,
        })
        .collect();
    LinearGraph::from_edges(rows, &edges)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::layout::layout_indexes;
    use crate::test_support::Random;

    const FIXTURES: &str = include_str!("testdata/linear_bek/bek.txt");
    const HOUR: i64 = 60 * 60 * 1000;
    const DAY: i64 = 24 * HOUR;

    fn fixture(name: &str) -> (&'static str, &'static str) {
        FIXTURES
            .split("# ")
            .find_map(|case| case.strip_prefix(name)?.strip_prefix('\n'))
            .and_then(|case| case.trim_end().split_once("\n=\n"))
            .unwrap()
    }

    fn sorted(graph: &LinearGraph, times: &[i64]) -> LinearGraph {
        let layout = layout_indexes(graph, &graph.heads());
        reordered(graph, &bek_order(graph, &layout, times))
    }

    fn matches_fixture(name: &str) {
        let (before, after) = fixture(name);
        let graph = LinearGraph::parse(before);
        let times = vec![0; graph.rows.len()];
        assert_eq!(sorted(&graph, &times).dump(), after);
    }

    fn order(parents: &[&[u32]], times: &[i64]) -> Vec<u32> {
        let graph = LinearGraph::from_parents(parents.iter().copied());
        sorted(&graph, times).rows
    }

    #[test]
    fn same_as_intellij_ea68497() {
        matches_fixture("ea68497");
    }

    #[test]
    fn same_as_intellij_simple_merge() {
        matches_fixture("simpleMerge");
    }

    #[test]
    fn same_as_intellij_two_heads() {
        matches_fixture("twoHeads");
    }

    #[test]
    fn same_as_intellij_simple_merge_with_two_heads() {
        matches_fixture("simpleMergeWithTwoHeads");
    }

    #[test]
    fn standard_leaves_what_was_merged_under_the_merge() {
        // The log interleaves by date the branch (2, 4) with the main one (1, 3).
        let parents: [&[u32]; 6] = [&[1, 2], &[3], &[4], &[5], &[5], &[]];
        let times = [6 * HOUR, 5 * HOUR, 4 * HOUR, 3 * HOUR, 2 * HOUR, HOUR];
        assert_eq!(order(&parents, &times), [0, 2, 4, 1, 3, 5]);
    }

    fn assert_children_first(graph: &LinearGraph) {
        for row in 0..graph.len() {
            for edge in graph.downs.of(row) {
                assert!(edge.up < edge.down, "{edge:?}");
            }
        }
    }

    #[test]
    fn standard_children_before_parents() {
        let parents: [&[u32]; 7] = [&[1, 4], &[2], &[3, 4], &[5, 6], &[5], &[6], &[]];
        let graph = LinearGraph::from_parents(parents.iter().copied());
        assert_children_first(&sorted(&graph, &[7, 6, 5, 4, 3, 2, 1]));
    }

    fn random_graph(random: &mut Random, size: u32) -> LinearGraph {
        let mut parents: Vec<Vec<u32>> = Vec::new();
        for row in 0..size {
            let left = size - row - 1;
            let mut own = Vec::new();
            if left > 0 && random.below(30) != 0 {
                for _ in 0..1 + random.below(10) / 7 {
                    let reach = if random.below(10) == 0 {
                        left
                    } else {
                        left.min(8)
                    };
                    own.push(row + 1 + random.below(reach));
                }
            }
            parents.push(own);
        }
        LinearGraph::from_parents(parents.iter().map(Vec::as_slice))
    }

    fn random_times(random: &mut Random, graph: &LinearGraph) -> Vec<i64> {
        let mut time = 1000 * DAY;
        (0..graph.len())
            .map(|_| {
                time -= match random.below(4) {
                    0 => 5 * DAY,
                    1 => 5 * HOUR,
                    _ => 0,
                };
                time
            })
            .collect()
    }

    #[test]
    fn standard_children_before_parents_in_random_graphs() {
        for seed in 0..500 {
            let mut random = Random(seed);
            let size = 2 + random.below(300);
            let graph = random_graph(&mut random, size);
            let times = random_times(&mut random, &graph);

            let result = sorted(&graph, &times);

            let mut rows = result.rows.clone();
            rows.sort_unstable();
            assert_eq!(rows, graph.rows, "seed {seed}");
            assert_children_first(&result);
        }
    }

    #[test]
    fn branch_block_is_not_split_with_less_than_three_days() {
        // By date they would go 2, 0, 3, 1, but each branch goes in whole.
        let parents: [&[u32]; 5] = [&[1], &[4], &[3], &[4], &[]];
        let times = [10 * DAY, 8 * DAY, 11 * DAY, 9 * DAY, DAY];
        assert_eq!(order(&parents, &times), [2, 3, 0, 1, 4]);
    }

    #[test]
    fn branch_is_split_after_more_than_three_days() {
        // Between 0 and 1 there are four days: the other branch goes in between.
        let parents: [&[u32]; 5] = [&[1], &[4], &[3], &[4], &[]];
        let times = [12 * DAY, 8 * DAY, 11 * DAY, 9 * DAY, DAY];
        assert_eq!(order(&parents, &times), [0, 2, 3, 1, 4]);
    }

    // A branch of 23 commits on root 24 and another of a single commit, 23, with a date just below
    // that of the 5-hour jump after row `gap`.
    fn with_gap(gap: usize) -> Vec<u32> {
        let mut parents: Vec<Vec<u32>> = (1..23).map(|row| vec![row]).collect();
        parents.push(vec![24]);
        parents.push(vec![24]);
        parents.push(Vec::new());
        let mut times = vec![100 * DAY; 25];
        for row in 1..23 {
            let step = if row == gap + 1 { 5 * HOUR } else { HOUR };
            times[row] = times[row - 1] - step;
        }
        times[23] = times[gap] - 2 * HOUR;
        times[24] = 0;
        let parents: Vec<&[u32]> = parents.iter().map(Vec::as_slice).collect();
        order(&parents, &times)
    }

    #[test]
    fn block_of_more_than_20_is_split_with_more_than_four_hours() {
        assert_eq!(with_gap(1)[..4], [0, 1, 23, 2]);
    }

    #[test]
    fn block_of_20_or_less_is_not_split_with_more_than_four_hours() {
        let expected: Vec<u32> = (0..25).collect();
        assert_eq!(with_gap(6), expected);
    }

    #[test]
    fn a_merge_does_not_split_the_block_even_if_months_pass() {
        let parents: [&[u32]; 4] = [&[1, 2], &[3], &[3], &[]];
        let times = [100 * DAY, 2 * DAY, 3 * DAY, DAY];
        assert_eq!(order(&parents, &times), [0, 2, 1, 3]);
    }
}
