use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap, HashSet, VecDeque};

use crate::log::collapse::{EdgeStorage, GraphTarget};
use crate::log::graph::{EdgeStyle, GraphEdge, LinearGraph};

pub const MAX_BLOCK_SIZE: u32 = 200;
// `PrintElementGeneratorImpl.LONG_EDGE_SIZE`.
const MAGIC_SET_SIZE: u32 = 30;
const MAX_COLLAPSED_MERGES: usize = 10;

// `java.util.PriorityQueue` ordered by the lower row. The tie-break matters: between two edges
// that reach the same row, only the first to come out makes its origin part of the block.
#[derive(Default)]
struct EdgeQueue(Vec<GraphEdge>);

impl EdgeQueue {
    fn push(&mut self, edge: GraphEdge) {
        let mut slot = self.0.len();
        self.0.push(edge);
        while slot > 0 {
            let parent = (slot - 1) / 2;
            if edge.down >= self.0[parent].down {
                break;
            }
            self.0[slot] = self.0[parent];
            slot = parent;
        }
        self.0[slot] = edge;
    }

    fn pop(&mut self) -> Option<GraphEdge> {
        let last = self.0.pop()?;
        let Some(first) = self.0.first().copied() else {
            return Some(last);
        };
        let size = self.0.len();
        let mut slot = 0;
        while slot < size / 2 {
            let mut child = 2 * slot + 1;
            if child + 1 < size && self.0[child].down > self.0[child + 1].down {
                child += 1;
            }
            if last.down <= self.0[child].down {
                break;
            }
            self.0[slot] = self.0[child];
            slot = child;
        }
        self.0[slot] = last;
        Some(first)
    }
}

// `LinearBekGraph`: what changes over the base graph, which is never touched.
#[derive(Clone, Debug, Default)]
struct LinearBekGraph {
    hidden: EdgeStorage,
    dotted: EdgeStorage,
}

impl LinearBekGraph {
    fn down_edges(&self, base: &LinearGraph, row: u32) -> Vec<GraphEdge> {
        let hidden = self.hidden.down_edges(row);
        let mut edges = self.dotted.down_edges(row);
        edges.extend_from_slice(base.downs.of(row));
        edges.retain(|edge| !hidden.contains(edge));
        edges
    }

    fn has_down(&self, base: &LinearGraph, up: u32, down: u32) -> bool {
        self.down_edges(base, up)
            .iter()
            .any(|edge| edge.down == down)
    }

    // A dotted edge that gets hidden is remembered as hidden, because `expand_edge` walks through it.
    fn remove_edge(&mut self, base: &LinearGraph, up: u32, down: u32) {
        let edge = dotted(up, down);
        if self.dotted.has_edge(up, down) {
            self.dotted.remove(edge);
            self.hidden.create(edge);
        } else if let Some(edge) = usual_edge(base, up, down) {
            self.hidden.create(edge);
        }
    }

    fn replace_edge(&mut self, base: &LinearGraph, up: u32, down: u32) {
        if !self.dotted.has_edge(up, down) {
            if let Some(edge) = usual_edge(base, up, down) {
                self.hidden.create(edge);
            }
            self.dotted.create(dotted(up, down));
        }
    }

    // The order does not change the result: the hidden edges reachable from `edge` are removed. It
    // is iterative because in Rust a stack overflow closes the application.
    fn expand_edge(&mut self, edge: GraphEdge) {
        let mut pending = vec![edge];
        while let Some(edge) = pending.pop() {
            self.dotted.remove(edge);
            let mut hidden = self.hidden.down_edges(edge.up);
            hidden.extend(self.hidden.up_edges(edge.down));
            for other in hidden {
                self.hidden.remove(other);
                if other.style == EdgeStyle::Dotted {
                    pending.push(other);
                }
            }
        }
    }
}

fn dotted(up: u32, down: u32) -> GraphEdge {
    GraphEdge {
        up,
        down,
        style: EdgeStyle::Dotted,
    }
}

fn usual_edge(base: &LinearGraph, up: u32, down: u32) -> Option<GraphEdge> {
    base.downs
        .of(up)
        .iter()
        .find(|edge| edge.down == down)
        .copied()
}

// `LinearBekGraphBuilder.MergeFragment`. The order of `tails` and `tail_edges` does not matter:
// when collapsing, each new edge comes from a different tail.
struct MergeFragment {
    parent: u32,
    left_child: u32,
    right_child: u32,
    merge_with_old_commit: bool,
    tail_edges: BTreeSet<(u32, u32)>,
    body: BTreeSet<u32>,
    tails: BTreeSet<u32>,
}

impl MergeFragment {
    fn new(parent: u32, left_child: u32, right_child: u32) -> Self {
        Self {
            parent,
            left_child,
            right_child,
            merge_with_old_commit: false,
            tail_edges: BTreeSet::new(),
            body: BTreeSet::new(),
            tails: BTreeSet::new(),
        }
    }

    fn add_tail(&mut self, tail: u32) {
        if !self.body.contains(&tail) {
            self.tails.insert(tail);
        }
    }

    fn add_tail_edge(&mut self, up: u32, down: u32) {
        if !self.body.contains(&up) {
            self.tails.insert(up);
            self.tail_edges.insert((up, down));
        }
    }

    fn has_tail_edge(&self, up: u32) -> bool {
        self.tail_edges.iter().any(|edge| edge.0 == up)
    }

    fn tails_and_body(&self) -> BTreeSet<u32> {
        self.body.union(&self.tails).copied().collect()
    }

    fn all_nodes(&self) -> BTreeSet<u32> {
        let mut nodes = self.tails_and_body();
        nodes.extend([self.parent, self.left_child, self.right_child]);
        nodes
    }

    fn collapse(&self, base: &LinearGraph, graph: &mut LinearBekGraph) {
        for (up, down) in &self.tail_edges {
            graph.remove_edge(base, *up, *down);
        }
        for tail in &self.tails {
            if graph.has_down(base, *tail, self.left_child) {
                graph.replace_edge(base, *tail, self.left_child);
            } else {
                graph.dotted.create(dotted(*tail, self.left_child));
            }
        }
        graph.remove_edge(base, self.parent, self.left_child);
    }
}

// `LinearBekGraphBuilder`. `layout` is per commit: the rows of `base` are already in BEK order.
struct Builder<'a> {
    base: &'a LinearGraph,
    layout: &'a [u32],
    graph: &'a LinearBekGraph,
}

impl Builder<'_> {
    fn layout_of(&self, row: u32) -> u32 {
        self.layout[self.base.rows[row as usize] as usize]
    }

    fn enqueue(&self, queue: &mut EdgeQueue, row: u32) {
        for edge in self.graph.down_edges(self.base, row) {
            queue.push(edge);
        }
    }

    fn fragment(&self, merge: u32) -> Option<MergeFragment> {
        let mut downs: Vec<u32> = self
            .graph
            .down_edges(self.base, merge)
            .iter()
            .map(|edge| edge.down)
            .collect();
        downs.sort_unstable();
        match downs[..] {
            [right, left] => self.fragment_between(left, right, merge),
            _ => None,
        }
    }

    fn fragment_between(&self, left: u32, right: u32, parent: u32) -> Option<MergeFragment> {
        let mut fragment = MergeFragment::new(parent, left, right);
        let left_li = self.layout_of(left);
        let right_li = self.layout_of(right);
        let mut rows_count = 1;
        let mut block_size = 1;
        let mut queue = EdgeQueue::default();
        self.enqueue(&mut queue, right);
        let mut magic_set: Option<HashSet<u32>> = None;
        while let Some(GraphEdge { up, down: next, .. }) = queue.pop() {
            if next == left {
                fragment.add_tail(up);
                fragment.merge_with_old_commit = true;
            } else if next == right + rows_count {
                rows_count += 1;
                block_size += 1;
                self.enqueue(&mut queue, next);
                fragment.body.insert(up);
            } else if next > right + rows_count && next < left {
                rows_count = next - right + 1;
                block_size += 1;
                self.enqueue(&mut queue, next);
                fragment.body.insert(up);
            } else if next > left {
                let li = self.layout_of(next);
                let mut magic = |builder: &Self| -> bool {
                    magic_set
                        .get_or_insert_with(|| builder.magic_set(left))
                        .contains(&next)
                };
                if left_li > right_li && !fragment.merge_with_old_commit {
                    if next > left + MAGIC_SET_SIZE || !magic(self) {
                        return None;
                    }
                    fragment.add_tail_edge(up, next);
                } else if (li > left_li && li < right_li) || li == left_li {
                    fragment.add_tail_edge(up, next);
                } else if li >= right_li {
                    return None;
                } else if next > left + MAGIC_SET_SIZE {
                    if !fragment.has_tail_edge(up) && !fragment.body.contains(&up) {
                        return None;
                    }
                } else if magic(self) {
                    fragment.add_tail_edge(up, next);
                } else {
                    return None;
                }
            }
            if block_size >= MAX_BLOCK_SIZE {
                return None;
            }
        }
        // Without tails, the initial import has been reached.
        (!fragment.tails.is_empty()).then_some(fragment)
    }

    fn magic_set(&self, node: u32) -> HashSet<u32> {
        let mut magic_set = HashSet::new();
        let mut queue = BinaryHeap::new();
        let enqueue = |queue: &mut BinaryHeap<Reverse<u32>>, row: u32| {
            for edge in self.graph.down_edges(self.base, row) {
                queue.push(Reverse(edge.down));
            }
        };
        enqueue(&mut queue, node);
        while let Some(Reverse(next)) = queue.pop() {
            if next > node + MAGIC_SET_SIZE {
                break;
            }
            magic_set.insert(next);
            enqueue(&mut queue, next);
        }
        magic_set
    }

    // From bottom to top: on reaching a merge, those nested under it are already collapsed.
    fn collapse_all(base: &LinearGraph, layout: &[u32]) -> (LinearBekGraph, HashSet<u32>) {
        let mut graph = LinearBekGraph::default();
        let mut merges = HashSet::new();
        for row in (0..base.len()).rev() {
            let fragment = Builder {
                base,
                layout,
                graph: &graph,
            }
            .fragment(row);
            if let Some(fragment) = fragment {
                fragment.collapse(base, &mut graph);
                merges.insert(fragment.parent);
            }
        }
        (graph, merges)
    }
}

// `LinearBekController`. `base` and `layout` are the view's: they do not change while it lives.
#[derive(Clone, Debug)]
pub struct LinearBekController {
    graph: LinearBekGraph,
    merges: HashSet<u32>,
}

impl LinearBekController {
    pub fn new(base: &LinearGraph, layout: &[u32]) -> Self {
        let (graph, merges) = Builder::collapse_all(base, layout);
        Self { graph, merges }
    }

    pub fn compile(&self, base: &LinearGraph) -> LinearGraph {
        let edges: Vec<GraphEdge> = (0..base.len())
            .flat_map(|row| self.graph.down_edges(base, row))
            .collect();
        LinearGraph::from_edges(base.rows.clone(), &edges)
    }

    // `merges` remains that of the freshly created view, as in IntelliJ.
    pub fn collapse_all(&mut self, base: &LinearGraph, layout: &[u32]) {
        self.graph = Builder::collapse_all(base, layout).0;
    }

    pub fn expand_all(&mut self) {
        self.graph = LinearBekGraph::default();
    }

    // What a click does. On an edge it only acts if it is dotted.
    pub fn click(&mut self, base: &LinearGraph, layout: &[u32], target: GraphTarget) -> bool {
        let edge = match target {
            GraphTarget::Node { row } => {
                if self.collapse_node(base, layout, row) {
                    return true;
                }
                self.dotted_edge(row)
            }
            GraphTarget::Edge { up, down, style } => {
                (style == EdgeStyle::Dotted).then(|| dotted(up, down))
            }
        };
        if let Some(edge) = edge {
            self.expand_edge(edge);
        }
        edge.is_some()
    }

    fn dotted_edge(&self, row: u32) -> Option<GraphEdge> {
        let hidden = self.graph.hidden.adjacent(row);
        let dotted = self.graph.dotted.adjacent(row);
        dotted.into_iter().find(|edge| !hidden.contains(edge))
    }

    // What is highlighted on hover, if there is anything to highlight.
    pub fn hover(
        &self,
        base: &LinearGraph,
        layout: &[u32],
        target: GraphTarget,
    ) -> Option<Vec<u32>> {
        let edge = match target {
            GraphTarget::Node { row } => {
                if let Some(rows) = self.highlight_node(base, layout, row) {
                    return Some(rows);
                }
                self.dotted_edge(row)
            }
            GraphTarget::Edge { up, down, style } => {
                (style == EdgeStyle::Dotted).then(|| dotted(up, down))
            }
        };
        edge.map(|edge| vec![edge.up, edge.down])
    }

    fn expand_edge(&mut self, edge: GraphEdge) {
        self.graph.expand_edge(edge);
    }

    fn is_collapsed(&self, base: &LinearGraph, merge: u32) -> bool {
        self.graph.down_edges(base, merge).len() == 1
    }

    // `iterateMerges` goes up the base graph row by row, the lowest first, and keeps the first
    // expanded merge that gives an answer. Its `MAX_BLOCK_SIZE` limit never acts: `BfsSearch` only
    // counts the results, and with one it already finishes.
    fn around_merge<T>(
        &self,
        base: &LinearGraph,
        node: u32,
        mut answer: impl FnMut(u32) -> Option<T>,
    ) -> Option<T> {
        let mut queue = BinaryHeap::from([node]);
        let mut visited = HashSet::new();
        while let Some(merge) = queue.pop() {
            if !visited.insert(merge) {
                continue;
            }
            if self.merges.contains(&merge) && !self.is_collapsed(base, merge) {
                if let Some(found) = answer(merge) {
                    return Some(found);
                }
            }
            queue.extend(base.ups.of(merge).iter().map(|edge| edge.up));
        }
        None
    }

    fn collapse_node(&mut self, base: &LinearGraph, layout: &[u32], node: u32) -> bool {
        let nodes = self.around_merge(base, node, |merge| {
            let fragments = self.child_fragments(base, layout, merge);
            let nodes: BTreeSet<u32> = fragments
                .iter()
                .flat_map(|fragment| {
                    let mut nodes = fragment.tails_and_body();
                    nodes.insert(fragment.parent);
                    nodes
                })
                .collect();
            let contains =
                nodes.contains(&node) || fragments.iter().any(|item| item.left_child == node);
            contains.then_some(nodes)
        });
        let Some(nodes) = nodes else {
            return false;
        };
        for row in nodes.into_iter().rev() {
            self.collapse_fragment(base, layout, row);
        }
        true
    }

    // `highlightNode`.
    fn highlight_node(&self, base: &LinearGraph, layout: &[u32], node: u32) -> Option<Vec<u32>> {
        self.around_merge(base, node, |merge| {
            let fragments = self.child_fragments(base, layout, merge);
            let nodes: BTreeSet<u32> = fragments
                .iter()
                .flat_map(MergeFragment::all_nodes)
                .collect();
            nodes
                .contains(&node)
                .then(|| nodes.into_iter().collect::<Vec<u32>>())
        })
    }

    fn builder<'a>(&'a self, base: &'a LinearGraph, layout: &'a [u32]) -> Builder<'a> {
        Builder {
            base,
            layout,
            graph: &self.graph,
        }
    }

    // IntelliJ enqueues the rows in the order of a `HashSet`; here, from top to bottom. It only
    // matters if there are more than `MAX_COLLAPSED_MERGES` nested: it decides which ones get in.
    fn child_fragments(
        &self,
        base: &LinearGraph,
        layout: &[u32],
        merge: u32,
    ) -> Vec<MergeFragment> {
        let builder = self.builder(base, layout);
        let mut result = Vec::new();
        let mut to_process = VecDeque::from([merge]);
        while let Some(next) = to_process.pop_front() {
            let Some(fragment) = builder.fragment(next) else {
                continue;
            };
            for row in fragment.tails_and_body() {
                if !to_process.contains(&row) {
                    to_process.push_back(row);
                }
            }
            result.push(fragment);
            if result.len() > MAX_COLLAPSED_MERGES {
                break;
            }
        }
        result
    }

    fn collapse_fragment(&mut self, base: &LinearGraph, layout: &[u32], merge: u32) {
        if let Some(fragment) = self.builder(base, layout).fragment(merge) {
            fragment.collapse(base, &mut self.graph);
        }
    }
}

// `LinearBekController` right after being created: every merge that can be, collapsed.
pub fn collapse_merges(graph: &LinearGraph, layout: &[u32]) -> LinearGraph {
    LinearBekController::new(graph, layout).compile(graph)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::layout::layout_indexes;

    const COLLAPSE: &str = include_str!("testdata/linear_bek/collapse.txt");
    const EXPAND: &str = include_str!("testdata/linear_bek/expand.txt");

    // Each case of the files: name, starting graph and expected graph.
    fn cases(
        text: &'static str,
    ) -> impl Iterator<Item = (&'static str, &'static str, &'static str)> {
        text.split("# ")
            .filter(|case| !case.is_empty())
            .map(|case| {
                let (name, graphs) = case.split_once('\n').unwrap();
                let (before, after) = graphs.trim_end().split_once("\n=\n").unwrap();
                (name, before, after)
            })
    }

    fn layout_of(graph: &LinearGraph) -> Vec<u32> {
        layout_indexes(graph, &graph.heads())
    }

    fn collapse_fixture(name: &str) {
        let (_, before, after) = cases(COLLAPSE).find(|case| case.0 == name).unwrap();
        let graph = LinearGraph::parse(before);
        assert_eq!(collapse_merges(&graph, &layout_of(&graph)).dump(), after);
    }

    macro_rules! same_as_intellij {
        ($($test:ident: $name:literal,)*) => {
            $(
                #[test]
                fn $test() {
                    collapse_fixture($name);
                }
            )*
        };
    }

    same_as_intellij! {
        same_as_intellij_simple_graph: "simpleGraphTest",
        same_as_intellij_two_sections: "twoSectionsGraphTest",
        same_as_intellij_recursive_sections: "recursiveSectionsTest",
        same_as_intellij_diagonal: "diagonalTest",
        same_as_intellij_different_diagonal: "differentDiagonalTest",
        same_as_intellij_complicated_branches: "complicatedBranchesTest",
        same_as_intellij_interesting: "interestingTest",
        same_as_intellij_interesting_2: "interestingTest2",
        same_as_intellij_small_recursive_branches: "smallRecursiveBranchesTest",
        same_as_intellij_two_tails_branch: "twoTailsBranchTest",
        same_as_intellij_incoming_edge: "incomingEdgeTest",
        same_as_intellij_hidden_incoming_edges: "hiddenIncomingEdges",
        same_as_intellij_merge_with_old_commit: "testMergeWithOldCommit",
        same_as_intellij_heads_order_1: "testHeadsOrder1",
        same_as_intellij_heads_order_2: "testHeadsOrder2",
        same_as_intellij_katisha: "testKatisha",
        same_as_intellij_initial_import: "testInitialImport",
        same_as_intellij_triangle: "testTriangle",
        same_as_intellij_reversed_parents: "reversedParentsTest",
        same_as_intellij_crossed_tails: "crossedTailsTest",
    }

    #[test]
    fn every_intellij_case_has_a_test() {
        assert_eq!(cases(COLLAPSE).count(), 20);
        assert_eq!(cases(EXPAND).count(), 2);
    }

    #[test]
    fn same_as_intellij_when_expanding_an_edge() {
        for (name, before, after) in cases(EXPAND) {
            let mut words = name.split(' ').skip(1).map(|word| word.parse().unwrap());
            let edge = dotted(words.next().unwrap(), words.next().unwrap());
            let graph = LinearGraph::parse(before);
            let mut controller = LinearBekController::new(&graph, &layout_of(&graph));

            controller.expand_edge(edge);

            assert_eq!(controller.compile(&graph).dump(), after, "{name}");
        }
    }

    #[test]
    fn same_as_intellij_when_collapsing_after_expanding() {
        let graph = LinearGraph::parse(
            "0(4, 1)\n1(2)\n2(6, 3)\n3(9)\n4(6, 5)\n5(8)\n6(8)\n7(8)\n8(9)\n9()",
        );
        let layout = layout_of(&graph);
        let mut controller = LinearBekController::new(&graph, &layout);

        controller.expand_edge(dotted(5, 6));
        controller.collapse_fragment(&graph, &layout, 4);
        controller.expand_edge(dotted(3, 4));

        assert_eq!(
            controller.compile(&graph).dump(),
            "0(4, 1)\n1(2)\n2(6, 3)\n3(9)\n4(5)\n5(6.dot)\n6(8)\n7(8)\n8(9)\n9()"
        );
    }

    #[test]
    fn same_as_intellij_when_replacing_a_dotted_edge() {
        let before = "0(3, 1)\n1(3, 2)\n2(4)\n3(4)\n4()";
        let graph = LinearGraph::parse(before);
        let mut controller = LinearBekController::new(&graph, &layout_of(&graph));

        controller.expand_edge(dotted(2, 3));

        assert_eq!(controller.compile(&graph).dump(), before);
    }

    #[test]
    fn linear_collapses_every_merge() {
        let graph = LinearGraph::parse("0(2, 1)\n1(3)\n2(3)\n3(5, 4)\n4(6)\n5(6)\n6()");

        let result = collapse_merges(&graph, &layout_of(&graph));

        for row in 0..result.len() {
            assert!(result.downs.of(row).len() < 2, "{}", result.dump());
        }
        assert_eq!(result.len(), graph.len());
    }

    // A merge at 0 with a branch of `size` commits: 1..=size. The first parent is `size + 1`.
    fn merge_with_block(size: u32) -> LinearGraph {
        let mut parents: Vec<Vec<u32>> = vec![vec![size + 1, 1]];
        parents.extend((1..size).map(|row| vec![row + 1]));
        parents.push(vec![size + 2]);
        parents.push(vec![size + 2]);
        parents.push(Vec::new());
        LinearGraph::from_parents(parents.iter().map(Vec::as_slice))
    }

    #[test]
    fn linear_respects_the_maximum_block() {
        for size in [MAX_BLOCK_SIZE, MAX_BLOCK_SIZE + 1] {
            let graph = merge_with_block(size);
            let result = collapse_merges(&graph, &layout_of(&graph));
            assert_eq!(result.dump(), graph.dump(), "block of {size}");
        }
    }

    #[test]
    fn linear_collapses_the_largest_allowed_block() {
        let graph = merge_with_block(MAX_BLOCK_SIZE - 1);
        let result = collapse_merges(&graph, &layout_of(&graph));
        assert_eq!(result.downs.of(0).len(), 1);
    }

    fn node(row: u32) -> GraphTarget {
        GraphTarget::Node { row }
    }

    #[test]
    fn click_on_a_node_with_a_dotted_edge_expands_it() {
        let graph = LinearGraph::parse("0(2, 1)\n1(3)\n2(3)\n3()");
        let layout = layout_of(&graph);
        let mut controller = LinearBekController::new(&graph, &layout);
        assert_eq!(
            controller.compile(&graph).dump(),
            "0(1)\n1(2.dot)\n2(3)\n3()"
        );

        assert!(controller.click(&graph, &layout, node(1)));

        assert_eq!(controller.compile(&graph).dump(), graph.dump());
    }

    #[test]
    fn click_on_an_expanded_merge_collapses_it_again() {
        let graph = LinearGraph::parse("0(2, 1)\n1(3)\n2(3)\n3()");
        let layout = layout_of(&graph);
        let mut controller = LinearBekController::new(&graph, &layout);
        let collapsed = controller.compile(&graph).dump();
        controller.click(&graph, &layout, node(1));

        assert!(controller.click(&graph, &layout, node(0)));

        assert_eq!(controller.compile(&graph).dump(), collapsed);
    }

    #[test]
    fn click_on_the_first_branch_of_an_expanded_merge_collapses_it() {
        // Row 2 is neither tail nor body: it counts because it is the fragment's left child.
        let graph = LinearGraph::parse("0(2, 1)\n1(3)\n2(3)\n3()");
        let layout = layout_of(&graph);
        let mut controller = LinearBekController::new(&graph, &layout);
        let collapsed = controller.compile(&graph).dump();
        controller.click(&graph, &layout, node(1));

        assert!(controller.click(&graph, &layout, node(2)));

        assert_eq!(controller.compile(&graph).dump(), collapsed);
    }

    #[test]
    fn click_on_a_row_without_merges_changes_nothing() {
        let graph = LinearGraph::parse("0(1)\n1(2)\n2()");
        let layout = layout_of(&graph);
        let mut controller = LinearBekController::new(&graph, &layout);

        assert!(!controller.click(&graph, &layout, node(1)));

        assert_eq!(controller.compile(&graph).dump(), graph.dump());
    }

    #[test]
    fn click_also_collapses_nested_merges() {
        let before = "0(5, 1)\n1(3, 2)\n2(4)\n3(4)\n4(6)\n5(6)\n6()";
        let graph = LinearGraph::parse(before);
        let layout = layout_of(&graph);
        let mut controller = LinearBekController::new(&graph, &layout);
        let collapsed = controller.compile(&graph).dump();
        controller.expand_all();
        assert_eq!(controller.compile(&graph).dump(), before);

        // Row 5 is the left child of 0: the fragment of 0 drags along that of 1, nested in it.
        assert!(controller.click(&graph, &layout, node(5)));

        assert_eq!(controller.compile(&graph).dump(), collapsed);
    }

    #[test]
    fn click_collapses_the_nearest_merge_containing_the_row() {
        let before = "0(5, 1)\n1(3, 2)\n2(4)\n3(4)\n4(6)\n5(6)\n6()";
        let graph = LinearGraph::parse(before);
        let layout = layout_of(&graph);
        let mut controller = LinearBekController::new(&graph, &layout);
        controller.expand_all();

        assert!(controller.click(&graph, &layout, node(2)));

        assert_eq!(
            controller.compile(&graph).dump(),
            "0(5, 1)\n1(2)\n2(3.dot)\n3(4)\n4(6)\n5(6)\n6()"
        );
    }

    fn dotted_target(up: u32, down: u32) -> GraphTarget {
        GraphTarget::Edge {
            up,
            down,
            style: EdgeStyle::Dotted,
        }
    }

    #[test]
    fn click_on_a_dotted_edge_expands_it() {
        let graph = LinearGraph::parse("0(2, 1)\n1(3)\n2(3)\n3()");
        let layout = layout_of(&graph);
        let mut controller = LinearBekController::new(&graph, &layout);

        assert!(controller.click(&graph, &layout, dotted_target(1, 2)));

        assert_eq!(controller.compile(&graph).dump(), graph.dump());
    }

    #[test]
    fn click_on_a_normal_edge_does_nothing() {
        let graph = LinearGraph::parse("0(2, 1)\n1(3)\n2(3)\n3()");
        let layout = layout_of(&graph);
        let mut controller = LinearBekController::new(&graph, &layout);
        let collapsed = controller.compile(&graph).dump();
        let usual = GraphTarget::Edge {
            up: 0,
            down: 1,
            style: EdgeStyle::Usual,
        };

        assert!(!controller.click(&graph, &layout, usual));

        assert_eq!(controller.compile(&graph).dump(), collapsed);
    }

    #[test]
    fn hover_on_an_expanded_merge_marks_its_whole_fragment() {
        let graph = LinearGraph::parse("0(2, 1)\n1(3)\n2(3)\n3()");
        let layout = layout_of(&graph);
        let mut controller = LinearBekController::new(&graph, &layout);
        controller.expand_all();

        let rows = controller.hover(&graph, &layout, node(1));

        // Parent, the two children and the tail.
        assert_eq!(rows, Some(vec![0, 1, 2]));
    }

    #[test]
    fn hover_marks_the_nearest_merge_containing_the_row() {
        let before = "0(5, 1)\n1(3, 2)\n2(4)\n3(4)\n4(6)\n5(6)\n6()";
        let graph = LinearGraph::parse(before);
        let layout = layout_of(&graph);
        let mut controller = LinearBekController::new(&graph, &layout);
        controller.expand_all();

        assert_eq!(
            controller.hover(&graph, &layout, node(2)),
            Some(vec![1, 2, 3])
        );
    }

    #[test]
    fn hover_ignores_an_already_collapsed_merge() {
        let graph = LinearGraph::parse("0(2, 1)\n1(3)\n2(3)\n3()");
        let layout = layout_of(&graph);
        let controller = LinearBekController::new(&graph, &layout);

        // The merge is collapsed: row 3 touches no dotted edge.
        assert_eq!(controller.hover(&graph, &layout, node(3)), None);
    }

    #[test]
    fn hover_on_a_node_with_a_dotted_edge_marks_its_ends() {
        let graph = LinearGraph::parse("0(2, 1)\n1(3)\n2(3)\n3()");
        let layout = layout_of(&graph);
        let controller = LinearBekController::new(&graph, &layout);

        assert_eq!(controller.hover(&graph, &layout, node(1)), Some(vec![1, 2]));
    }

    #[test]
    fn hover_on_edge_only_if_dotted() {
        let graph = LinearGraph::parse("0(2, 1)\n1(3)\n2(3)\n3()");
        let layout = layout_of(&graph);
        let controller = LinearBekController::new(&graph, &layout);
        let usual = GraphTarget::Edge {
            up: 0,
            down: 1,
            style: EdgeStyle::Usual,
        };

        assert_eq!(controller.hover(&graph, &layout, usual), None);
        assert_eq!(
            controller.hover(&graph, &layout, dotted_target(1, 2)),
            Some(vec![1, 2])
        );
    }

    #[test]
    fn buttons_expand_and_collapse_everything_again() {
        let graph = LinearGraph::parse("0(2, 1)\n1(3)\n2(3)\n3(5, 4)\n4(6)\n5(6)\n6()");
        let layout = layout_of(&graph);
        let mut controller = LinearBekController::new(&graph, &layout);
        let collapsed = controller.compile(&graph).dump();

        controller.expand_all();
        assert_eq!(controller.compile(&graph).dump(), graph.dump());

        controller.click(&graph, &layout, node(0));
        controller.collapse_all(&graph, &layout);
        assert_eq!(controller.compile(&graph).dump(), collapsed);
    }

    #[test]
    fn queue_breaks_ties_like_java() {
        // Same destination in all of them: they come out in the order of `PriorityQueue`'s heap.
        let edge = |up: u32, down: u32| GraphEdge {
            up,
            down,
            style: EdgeStyle::Usual,
        };
        let mut queue = EdgeQueue::default();
        for up in [1, 2, 3, 4, 5] {
            queue.push(edge(up, 9));
        }
        let mut order = Vec::new();
        while let Some(next) = queue.pop() {
            order.push(next.up);
        }
        assert_eq!(order, [1, 5, 4, 3, 2]);
    }

    #[test]
    fn queue_comes_out_like_java_with_insertions_and_removals() {
        let edge = |(up, down): (u32, u32)| GraphEdge {
            up,
            down,
            style: EdgeStyle::Usual,
        };
        let mut queue = EdgeQueue::default();
        for pair in [(1, 5), (2, 3), (3, 5), (4, 3), (5, 7), (6, 3), (7, 5)] {
            queue.push(edge(pair));
        }
        queue.pop();
        queue.push(edge((8, 3)));
        queue.push(edge((9, 2)));
        let mut order = Vec::new();
        while let Some(next) = queue.pop() {
            order.push((next.up, next.down));
        }
        // Output of `java.util.PriorityQueue` with those same operations.
        assert_eq!(
            order,
            [
                (9, 2),
                (4, 3),
                (8, 3),
                (6, 3),
                (1, 5),
                (7, 5),
                (3, 5),
                (5, 7)
            ]
        );
    }
}
