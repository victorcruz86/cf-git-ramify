use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use serde::Deserialize;

use crate::log::filter::BitVec;
use crate::log::graph::{EdgeStyle, GraphEdge, LinearGraph};
use crate::log::linear_bek::LinearBekController;
use crate::log::view::View;

const SHORT_FRAGMENT_MAX_SIZE: usize = 10;
const MAX_SEARCH_SIZE: usize = 10;
// `getPartLongFragment`: the hover does not walk a whole branch.
const HOVER_BOUND: u32 = 500;

// What is under the mouse, in view rows: the `getGraphElement` of a `PrintElement`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum GraphTarget {
    Node {
        row: u32,
    },
    Edge {
        up: u32,
        down: u32,
        style: EdgeStyle,
    },
}

impl GraphTarget {
    // The frontend points at what it painted, and the view may have changed since then.
    fn exists_in(self, graph: &LinearGraph) -> bool {
        match self {
            Self::Node { row } => row < graph.len(),
            Self::Edge { up, down, style } => {
                up < graph.len()
                    && graph
                        .downs
                        .of(up)
                        .iter()
                        .any(|edge| edge.down == down && edge.style == style)
            }
        }
    }

    fn ends(self) -> (u32, u32) {
        match self {
            Self::Node { row } => (row, row),
            Self::Edge { up, down, .. } => (up, down),
        }
    }

    // `CollapsedActionManager.getDottedEdge`.
    fn dotted_edge(self, graph: &LinearGraph) -> Option<GraphEdge> {
        match self {
            Self::Node { row } => graph
                .ups
                .of(row)
                .iter()
                .chain(graph.downs.of(row))
                .find(|edge| edge.style == EdgeStyle::Dotted)
                .copied(),
            Self::Edge { up, down, style } => {
                (style == EdgeStyle::Dotted).then(|| dotted(up, down))
            }
        }
    }

    // `CollapsedController.convertToDelegate`.
    fn in_delegate(self, shown: &CollapsedGraph) -> Self {
        let row = |row: u32| shown.delegate_rows[row as usize];
        match self {
            Self::Node { row: node } => Self::Node { row: row(node) },
            Self::Edge { up, down, style } => Self::Edge {
                up: row(up),
                down: row(down),
                style,
            },
        }
    }
}

// `EdgeStorage`: each edge is stored at both ends, in order of arrival and without repeating.
#[derive(Clone, Debug, Default)]
pub struct EdgeStorage {
    edges: HashMap<u32, Vec<(u32, EdgeStyle)>>,
}

impl EdgeStorage {
    fn put(&mut self, key: u32, value: (u32, EdgeStyle)) {
        let values = self.edges.entry(key).or_default();
        if !values.contains(&value) {
            values.push(value);
        }
    }

    fn take(&mut self, key: u32, value: (u32, EdgeStyle)) {
        let Some(values) = self.edges.get_mut(&key) else {
            return;
        };
        values.retain(|other| *other != value);
        if values.is_empty() {
            self.edges.remove(&key);
        }
    }

    pub fn create(&mut self, edge: GraphEdge) {
        self.put(edge.up, (edge.down, edge.style));
        self.put(edge.down, (edge.up, edge.style));
    }

    pub fn remove(&mut self, edge: GraphEdge) {
        self.take(edge.up, (edge.down, edge.style));
        self.take(edge.down, (edge.up, edge.style));
    }

    pub fn has_edge(&self, up: u32, down: u32) -> bool {
        self.edges
            .get(&up)
            .is_some_and(|values| values.iter().any(|value| value.0 == down))
    }

    // The edges that touch `row`, upwards or downwards.
    pub fn adjacent(&self, row: u32) -> Vec<GraphEdge> {
        let values = self.edges.get(&row).map_or(&[][..], Vec::as_slice);
        values
            .iter()
            .map(|(other, style)| GraphEdge {
                up: row.min(*other),
                down: row.max(*other),
                style: *style,
            })
            .collect()
    }

    pub fn down_edges(&self, row: u32) -> Vec<GraphEdge> {
        let mut edges = self.adjacent(row);
        edges.retain(|edge| edge.up == row);
        edges
    }

    pub fn up_edges(&self, row: u32) -> Vec<GraphEdge> {
        let mut edges = self.adjacent(row);
        edges.retain(|edge| edge.down == row);
        edges
    }
}

// `CollapsedGraph`: which rows of `delegate` are shown and which dotted edges join them.
// Everything is in rows of `delegate`, except the index of `delegate_rows`, which are the rows
// that are drawn. `matched` is the initial visibility: what the filter lets through and what the
// buttons go back to.
#[derive(Clone, Debug)]
pub struct CollapsedGraph {
    matched: Vec<bool>,
    visible: Vec<bool>,
    edges: EdgeStorage,
    delegate_rows: Vec<u32>,
}

impl CollapsedGraph {
    pub fn matching(delegate: &LinearGraph, matched: Vec<bool>) -> Self {
        let mut graph = Self {
            visible: matched.clone(),
            matched,
            edges: EdgeStorage::default(),
            delegate_rows: Vec::new(),
        };
        graph.update_rows(delegate);
        graph
    }

    fn all(delegate: &LinearGraph) -> Self {
        Self::matching(delegate, vec![true; delegate.rows.len()])
    }

    pub fn of_commits(delegate: &LinearGraph, kept: &BitVec) -> Self {
        let matched = delegate.rows.iter().map(|commit| kept.get(*commit));
        Self::matching(delegate, matched.collect())
    }

    fn update_rows(&mut self, delegate: &LinearGraph) {
        self.delegate_rows = (0..delegate.len())
            .filter(|row| self.visible[*row as usize])
            .collect();
    }

    pub fn add_dotted(&mut self, up: u32, down: u32) {
        self.edges.create(dotted(up, down));
    }

    // `removeAdditionalEdges` y `resetNodesVisibility`.
    fn reset(&mut self, delegate: &LinearGraph) {
        self.edges = EdgeStorage::default();
        self.visible.clone_from(&self.matched);
        self.update_rows(delegate);
    }

    fn shows_all(&self) -> bool {
        self.delegate_rows.len() == self.visible.len()
    }

    pub fn compile(&mut self, delegate: &LinearGraph) -> LinearGraph {
        let mut compiled_rows = vec![0; self.visible.len()];
        self.update_rows(delegate);
        for (compiled, row) in self.delegate_rows.iter().enumerate() {
            compiled_rows[*row as usize] = compiled as u32;
        }
        let mut edges = Vec::with_capacity(self.delegate_rows.len());
        for row in &self.delegate_rows {
            let stored = self.edges.down_edges(*row);
            for edge in delegate.downs.of(*row).iter().chain(&stored) {
                if self.visible[edge.down as usize] {
                    edges.push(GraphEdge {
                        up: compiled_rows[edge.up as usize],
                        down: compiled_rows[edge.down as usize],
                        style: edge.style,
                    });
                }
            }
        }
        let rows = self
            .delegate_rows
            .iter()
            .map(|row| delegate.rows[*row as usize])
            .collect();
        LinearGraph::from_edges(rows, &edges)
    }

    // `COLLAPSE_ALL` of `CollapsedActionManager`. It also walks what the filter hides: the
    // fragments come from the whole graph, and only what this same walk hides is skipped.
    fn collapse_all(&mut self, delegate: &LinearGraph, branches: &[bool]) {
        let fragments = LinearFragments {
            graph: delegate,
            branches,
        };
        self.reset(delegate);
        let mut hidden = vec![false; self.visible.len()];
        for row in 0..delegate.len() {
            if hidden[row as usize] {
                continue;
            }
            if let Some(fragment) = fragments.long_down_fragment(row) {
                for middle in middle_nodes(delegate, fragment.up, fragment.down, true) {
                    hidden[middle as usize] = true;
                    self.visible[middle as usize] = false;
                }
                self.add_dotted(fragment.up, fragment.down);
            }
        }
    }

    // `LINEAR_EXPAND_CASE`.
    fn expand(&mut self, delegate: &LinearGraph, edge: GraphEdge) {
        for middle in middle_nodes(delegate, edge.up, edge.down, true) {
            self.visible[middle as usize] = true;
        }
        self.edges.remove(edge);
    }

    // `LINEAR_COLLAPSE_CASE`, on the graph that is shown: it also collapses what was already
    // collapsed.
    fn collapse(&mut self, compiled: &LinearGraph, branches: &[bool], target: GraphTarget) -> bool {
        let fragments = LinearFragments {
            graph: compiled,
            branches,
        };
        let Some(fragment) = fragments.long_fragment(target) else {
            return false;
        };
        let delegate_row = |row: u32| self.delegate_rows[row as usize];
        let mut replaced = Vec::new();
        let mut hidden = Vec::new();
        for middle in middle_nodes(compiled, fragment.up, fragment.down, true) {
            hidden.push(delegate_row(middle));
            let edges = compiled
                .ups
                .of(middle)
                .iter()
                .chain(compiled.downs.of(middle));
            for edge in edges.filter(|edge| edge.style == EdgeStyle::Dotted) {
                replaced.push(dotted(delegate_row(edge.up), delegate_row(edge.down)));
            }
        }
        let new_edge = dotted(delegate_row(fragment.up), delegate_row(fragment.down));
        for row in hidden {
            self.visible[row as usize] = false;
        }
        self.edges.create(new_edge);
        for edge in replaced {
            self.edges.remove(edge);
        }
        true
    }
}

fn dotted(up: u32, down: u32) -> GraphEdge {
    GraphEdge {
        up,
        down,
        style: EdgeStyle::Dotted,
    }
}

// What is collapsed in a view: the linear branches of `CollapsedController` or the merges of
// `LinearBekController`, depending on the order. With a branch filter, the merges carry on top
// what `BranchFilterController` lets through. `FilteredController` supports no action.
#[derive(Clone)]
pub enum Collapse {
    Branches(CollapsedGraph),
    Merges(LinearBekController, Option<CollapsedGraph>),
    Filtered,
}

impl Collapse {
    pub fn branches(delegate: &LinearGraph) -> Self {
        Self::Branches(CollapsedGraph::all(delegate))
    }
}

fn compile_merges(
    controller: &LinearBekController,
    shown: &mut Option<CollapsedGraph>,
    delegate: &LinearGraph,
) -> LinearGraph {
    let graph = controller.compile(delegate);
    match shown {
        Some(shown) => shown.compile(&graph),
        None => graph,
    }
}

// The buttons `Collapse Linear Branches` and `Collapse Merges`. They always start from scratch,
// even if something was collapsed by hand.
pub fn collapse_all(view: &mut View) {
    let delegate = view.delegate.clone();
    let graph = match &mut view.collapse {
        Collapse::Branches(collapsed) => {
            collapsed.collapse_all(&delegate, &view.permanent.branches);
            collapsed.compile(&delegate)
        }
        Collapse::Merges(controller, shown) => {
            controller.collapse_all(&delegate, &view.permanent.layout);
            compile_merges(controller, shown, &delegate)
        }
        Collapse::Filtered => return,
    };
    view.spec.collapsed = true;
    view.set_graph(Arc::new(graph));
}

pub fn expand_all(view: &mut View) {
    let delegate = view.delegate.clone();
    let graph = match &mut view.collapse {
        Collapse::Branches(collapsed) => {
            collapsed.reset(&delegate);
            if collapsed.shows_all() {
                delegate
            } else {
                Arc::new(collapsed.compile(&delegate))
            }
        }
        Collapse::Merges(controller, shown) => {
            controller.expand_all();
            Arc::new(compile_merges(controller, shown, &delegate))
        }
        Collapse::Filtered => return,
    };
    view.spec.collapsed = false;
    view.set_graph(graph);
}

// A click on a node or an edge. It says whether the graph changed. After the button, in IntelliJ
// the dotted edge of a node comes from a `HashSet` whose hash changes on each run; here the upper
// one rules.
pub fn toggle(view: &mut View, target: GraphTarget) -> bool {
    if !target.exists_in(&view.graph) {
        return false;
    }
    let delegate = view.delegate.clone();
    let graph = match &mut view.collapse {
        Collapse::Branches(collapsed) => {
            match target.dotted_edge(&view.graph) {
                Some(edge) => {
                    let delegate_row = |row: u32| collapsed.delegate_rows[row as usize];
                    let edge = dotted(delegate_row(edge.up), delegate_row(edge.down));
                    collapsed.expand(&delegate, edge);
                }
                None => {
                    if !collapsed.collapse(&view.graph, &view.permanent.branches, target) {
                        return false;
                    }
                }
            }
            collapsed.compile(&delegate)
        }
        Collapse::Merges(controller, shown) => {
            let target = shown
                .as_ref()
                .map_or(target, |shown| target.in_delegate(shown));
            if !controller.click(&delegate, &view.permanent.layout, target) {
                return false;
            }
            compile_merges(controller, shown, &delegate)
        }
        Collapse::Filtered => return false,
    };
    view.set_graph(Arc::new(graph));
    true
}

// `MOUSE_OVER`: the rows that are highlighted, and that the cursor is the hand. `None` is the
// default answer: nothing highlighted and the normal cursor.
pub fn hover(view: &View, target: GraphTarget) -> Option<Vec<u32>> {
    if !target.exists_in(&view.graph) {
        return None;
    }
    let (delegate, layout) = (&view.delegate, &view.permanent.layout);
    match &view.collapse {
        Collapse::Branches(_) => hover_fragment(&view.graph, &view.permanent.branches, target),
        Collapse::Merges(controller, None) => controller.hover(delegate, layout, target),
        Collapse::Merges(controller, Some(shown)) => {
            let rows = controller.hover(delegate, layout, target.in_delegate(shown))?;
            let visible = rows
                .iter()
                .filter_map(|row| shown.delegate_rows.binary_search(row).ok());
            Some(visible.map(|row| row as u32).collect())
        }
        Collapse::Filtered => None,
    }
}

fn hover_fragment(graph: &LinearGraph, branches: &[bool], target: GraphTarget) -> Option<Vec<u32>> {
    if let Some(edge) = target.dotted_edge(graph) {
        return Some(vec![edge.up, edge.down]);
    }
    let fragments = LinearFragments { graph, branches };
    let fragment = fragments.part_long_fragment(target)?;
    let middle = middle_nodes(graph, fragment.up, fragment.down, false);
    Some(middle.into_iter().collect())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fragment {
    up: u32,
    down: u32,
}

// `LinearFragmentGenerator`. `branches` is per commit: its commits do not end up inside a
// fragment.
struct LinearFragments<'a> {
    graph: &'a LinearGraph,
    branches: &'a [bool],
}

impl LinearFragments<'_> {
    fn is_pinned(&self, row: u32) -> bool {
        self.branches[self.graph.rows[row as usize] as usize]
    }

    fn up_nodes(&self, row: u32) -> Vec<u32> {
        self.graph.ups.of(row).iter().map(|edge| edge.up).collect()
    }

    fn down_nodes(&self, row: u32) -> Vec<u32> {
        self.graph
            .downs
            .of(row)
            .iter()
            .map(|edge| edge.down)
            .collect()
    }

    fn next_nodes(&self, row: u32, is_down: bool) -> Vec<u32> {
        if is_down {
            self.down_nodes(row)
        } else {
            self.up_nodes(row)
        }
    }

    // `getFragment`: advances in topological order from `start` until everything converges on a
    // single row. IntelliJ walks `gray` in the order of a `HashSet`, but if there is a convergence
    // within 10 rows every order reaches it, and if not, none does.
    fn fragment(&self, start: u32, is_down: bool) -> Option<Fragment> {
        let mut black = vec![start];
        let mut gray = Vec::new();
        add_new(&mut gray, self.next_nodes(start, is_down));
        while black.len() < SHORT_FRAGMENT_MAX_SIZE {
            let position = gray.iter().position(|row| {
                let previous = self.next_nodes(*row, !is_down);
                previous.iter().all(|row| black.contains(row))
            })?;
            let next_black = gray[position];
            if gray.len() == 1 {
                return Some(if is_down {
                    Fragment {
                        up: start,
                        down: next_black,
                    }
                } else {
                    Fragment {
                        up: next_black,
                        down: start,
                    }
                });
            }
            let next_gray = self.next_nodes(next_black, is_down);
            if next_gray.is_empty() || self.is_pinned(next_black) {
                return None;
            }
            black.push(next_black);
            gray.remove(position);
            add_new(&mut gray, next_gray);
        }
        None
    }

    fn down_fragment(&self, row: u32) -> Option<Fragment> {
        self.fragment(row, true)
    }

    fn up_fragment(&self, row: u32) -> Option<Fragment> {
        self.fragment(row, false)
    }

    fn long_down_fragment(&self, row: u32) -> Option<Fragment> {
        self.long(self.down_fragment(row), u32::MAX)
    }

    fn long_fragment(&self, target: GraphTarget) -> Option<Fragment> {
        self.long(self.relative_fragment(target), u32::MAX)
    }

    fn part_long_fragment(&self, target: GraphTarget) -> Option<Fragment> {
        self.long(self.relative_fragment(target), HOVER_BOUND)
    }

    // `getRelativeFragment`: the fragment that contains the node or the edge, searching upwards.
    fn relative_fragment(&self, target: GraphTarget) -> Option<Fragment> {
        let (mut up, down) = target.ends();
        for _ in 0..MAX_SEARCH_SIZE {
            if let Some(fragment) = self.down_fragment(up).filter(|found| found.down >= down) {
                return Some(fragment);
            }
            match self.up_nodes(up)[..] {
                [only] => up = only,
                _ => break,
            }
        }
        None
    }

    // `getLongFragment`: joins fragments on both sides until it finds a branch.
    fn long(&self, start: Option<Fragment>, bound: u32) -> Option<Fragment> {
        let start = start?;
        let mut max_down = start.down;
        while let Some(fragment) = self.down_fragment(max_down) {
            if self.is_pinned(max_down) {
                break;
            }
            max_down = fragment.down;
            if max_down - start.down > bound {
                break;
            }
        }
        let mut max_up = start.up;
        while let Some(fragment) = self.up_fragment(max_up) {
            if self.is_pinned(max_up) {
                break;
            }
            max_up = fragment.up;
            if start.up - max_up > bound {
                break;
            }
        }
        if max_up != start.up || max_down != start.down {
            Some(Fragment {
                up: max_up,
                down: max_down,
            })
        } else {
            // A lone edge is not collapsed.
            (self.graph.downs.of(start.up).len() != 1).then_some(start)
        }
    }
}

// A row can arrive twice: a real edge and a dotted one between the same two.
fn add_new(rows: &mut Vec<u32>, new: Vec<u32>) {
    for row in new {
        if !rows.contains(&row) {
            rows.push(row);
        }
    }
}

// `FragmentGenerator.getWalkNodes`, without leaving `up..=down`.
fn walk_nodes(graph: &LinearGraph, start: u32, up: u32, down: u32, is_up: bool) -> BTreeSet<u32> {
    let mut nodes = BTreeSet::new();
    let mut walker = BTreeSet::from([start]);
    while let Some(next) = if is_up {
        walker.pop_last()
    } else {
        walker.pop_first()
    } {
        if (is_up && next >= up) || (!is_up && next <= down) {
            nodes.insert(next);
            if is_up {
                walker.extend(graph.ups.of(next).iter().map(|edge| edge.up));
            } else {
                walker.extend(graph.downs.of(next).iter().map(|edge| edge.down));
            }
        }
    }
    nodes
}

// `FragmentGenerator.getMiddleNodes`: what is at once below `up` and above `down`.
fn middle_nodes(graph: &LinearGraph, up: u32, down: u32, strict: bool) -> BTreeSet<u32> {
    let walk_down = walk_nodes(graph, up, up, down, false);
    let walk_up = walk_nodes(graph, down, up, down, true);
    let mut middle: BTreeSet<u32> = walk_down.intersection(&walk_up).copied().collect();
    if strict {
        middle.remove(&up);
        middle.remove(&down);
    }
    middle
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::view::{Permanent, SortType, ViewSpec};

    fn parents_of(parents: &[Vec<u32>]) -> LinearGraph {
        LinearGraph::from_parents(parents.iter().map(Vec::as_slice))
    }

    fn view_with_branches(graph: LinearGraph, branches: &[u32], sort: SortType) -> View {
        let heads = graph.heads();
        let colors = vec![0; heads.len()];
        let size = graph.rows.len();
        let mut flags = vec![false; size];
        for row in branches {
            flags[*row as usize] = true;
        }
        let permanent = Permanent::new(graph, &heads, colors, vec![0; size], flags);
        let spec = ViewSpec {
            sort,
            ..ViewSpec::default()
        };
        View::new(Arc::new(permanent), &spec)
    }

    fn view_of(graph: LinearGraph, branches: &[u32]) -> View {
        view_with_branches(graph, branches, SortType::Off)
    }

    // A chain of `length` commits, 1..=length, under a merge whose second parent is another
    // root, and which ends at a fork: row `length + 2`, with another child.
    fn chain_between_merge_and_fork(length: u32) -> LinearGraph {
        let root = length + 3;
        let fork = length + 2;
        let mut parents = vec![vec![1, root]];
        parents.extend((1..length).map(|row| vec![row + 1]));
        parents.push(vec![fork]);
        parents.push(vec![fork]);
        parents.push(Vec::new());
        parents.push(Vec::new());
        parents_of(&parents)
    }

    // A commit with `arms` parents, all on the same root.
    fn octopus(arms: u32) -> LinearGraph {
        let root = arms + 1;
        let mut parents = vec![(1..=arms).collect::<Vec<u32>>()];
        parents.extend((0..arms).map(|_| vec![root]));
        parents.push(Vec::new());
        parents_of(&parents)
    }

    fn dump(view: &View) -> String {
        view.graph.dump()
    }

    fn node(row: u32) -> GraphTarget {
        GraphTarget::Node { row }
    }

    fn edge(up: u32, down: u32, style: EdgeStyle) -> GraphTarget {
        GraphTarget::Edge { up, down, style }
    }

    #[test]
    fn collapse_all_collapses_linear_fragments() {
        let mut view = view_of(chain_between_merge_and_fork(5), &[]);

        collapse_all(&mut view);

        assert_eq!(dump(&view), "0(1, 8)\n1(5.dot)\n5(7)\n6(7)\n7()\n8()",);
        assert!(view.spec.collapsed);
    }

    #[test]
    fn collapse_does_not_collapse_commits_with_a_reference() {
        // The branch at 3 splits the chain into two fragments: the commit is visible.
        let mut view = view_of(chain_between_merge_and_fork(5), &[3]);

        collapse_all(&mut view);

        assert_eq!(
            dump(&view),
            "0(1, 8)\n1(3.dot)\n3(5.dot)\n5(7)\n6(7)\n7()\n8()",
        );
    }

    #[test]
    fn collapse_collapses_over_a_branch_at_the_end() {
        let mut view = view_of(chain_between_merge_and_fork(5), &[5]);

        collapse_all(&mut view);

        assert_eq!(dump(&view), "0(1, 8)\n1(5.dot)\n5(7)\n6(7)\n7()\n8()",);
    }

    #[test]
    fn collapse_does_not_collapse_10_or_more() {
        let mut view = view_of(octopus(9), &[]);
        let before = dump(&view);

        collapse_all(&mut view);

        assert_eq!(dump(&view), before);
    }

    #[test]
    fn collapse_collapses_a_fragment_of_9() {
        let mut view = view_of(octopus(8), &[]);

        collapse_all(&mut view);

        assert_eq!(dump(&view), "0(9.dot)\n9()");
    }

    #[test]
    fn collapse_collapses_chains_of_10_or_more() {
        // The limit is per short fragment: a chain is many one-step ones, chained.
        let mut view = view_of(chain_between_merge_and_fork(30), &[]);

        collapse_all(&mut view);

        assert_eq!(view.graph.len(), 6);
        assert_eq!(view.graph.downs.of(1)[0].style, EdgeStyle::Dotted);
    }

    #[test]
    fn collapse_does_not_collapse_a_lone_edge() {
        let mut view = view_of(LinearGraph::parse("0(1)\n1()"), &[]);

        collapse_all(&mut view);

        assert_eq!(dump(&view), "0(1)\n1()");
    }

    #[test]
    fn collapse_collapses_a_whole_diamond() {
        let mut view = view_of(LinearGraph::parse("0(1, 2)\n1(3)\n2(3)\n3()"), &[]);

        collapse_all(&mut view);

        assert_eq!(dump(&view), "0(3.dot)\n3()");
    }

    #[test]
    fn collapse_keeps_the_real_edge_next_to_the_dotted_one() {
        let mut view = view_of(LinearGraph::parse("0(2, 1)\n1(2)\n2()"), &[]);

        collapse_all(&mut view);

        assert_eq!(dump(&view), "0(2, 2.dot)\n2()");
    }

    #[test]
    fn collapse_does_not_pass_through_a_commit_without_parents() {
        // Arm 2 ends at a root: the merge cannot be collapsed.
        let mut view = view_of(LinearGraph::parse("0(1, 2)\n1(3)\n2()\n3()"), &[]);
        let before = dump(&view);

        collapse_all(&mut view);

        assert_eq!(dump(&view), before);
    }

    #[test]
    fn expand_all_returns_to_the_original_graph() {
        let graph = chain_between_merge_and_fork(5);
        let before = graph.dump();
        let mut view = view_of(graph, &[3]);
        collapse_all(&mut view);

        expand_all(&mut view);

        assert_eq!(dump(&view), before);
        assert!(!view.spec.collapsed);
    }

    #[test]
    fn click_collapses_and_expands_a_fragment() {
        let graph = chain_between_merge_and_fork(5);
        let before = graph.dump();
        let mut view = view_of(graph, &[]);

        assert!(toggle(&mut view, node(3)));
        assert_eq!(dump(&view), "0(1, 8)\n1(5.dot)\n5(7)\n6(7)\n7()\n8()");

        // Row 1 is now that of commit 5: it has the dotted edge.
        assert!(toggle(&mut view, node(1)));
        assert_eq!(dump(&view), before);
    }

    #[test]
    fn click_outside_a_fragment_changes_nothing() {
        let graph = LinearGraph::parse("0(1, 2)\n1()\n2()");
        let before = graph.dump();
        let mut view = view_of(graph, &[]);

        assert!(!toggle(&mut view, node(2)));
        assert!(!toggle(&mut view, node(99)));

        assert_eq!(dump(&view), before);
    }

    #[test]
    fn click_on_a_branch_between_two_fragments_expands_one() {
        let mut view = view_of(chain_between_merge_and_fork(5), &[3]);
        collapse_all(&mut view);
        assert_eq!(view.graph.len(), 7);

        // Commit 3 has two dotted edges: the upper one is expanded.
        assert!(toggle(&mut view, node(2)));

        assert_eq!(
            dump(&view),
            "0(1, 8)\n1(2)\n2(3)\n3(5.dot)\n5(7)\n6(7)\n7()\n8()"
        );
    }

    #[test]
    fn click_collapses_even_if_there_are_dotted_edges_inside() {
        // An arm of 12 commits: the diamond does not fit in a fragment until that arm is collapsed.
        let mut parents = vec![vec![1, 13]];
        parents.extend((1..12).map(|row| vec![row + 1]));
        parents.push(vec![14]);
        parents.push(vec![14]);
        parents.push(Vec::new());
        let graph = parents_of(&parents);
        let before = graph.dump();
        let mut view = view_of(graph, &[]);
        collapse_all(&mut view);
        assert_eq!(dump(&view), "0(1, 13)\n1(12.dot)\n12(14)\n13(14)\n14()");

        assert!(toggle(&mut view, node(0)));
        assert_eq!(dump(&view), "0(14.dot)\n14()");

        assert!(toggle(&mut view, node(0)));
        assert_eq!(dump(&view), before);
    }

    #[test]
    fn click_collapses_again_only_up_to_the_branch() {
        let mut view = view_of(chain_between_merge_and_fork(5), &[3]);
        collapse_all(&mut view);
        assert!(toggle(&mut view, node(1)));
        assert_eq!(
            dump(&view),
            "0(1, 8)\n1(2)\n2(3)\n3(5.dot)\n5(7)\n6(7)\n7()\n8()"
        );

        assert!(toggle(&mut view, node(2)));

        assert_eq!(
            dump(&view),
            "0(1, 8)\n1(3.dot)\n3(5.dot)\n5(7)\n6(7)\n7()\n8()"
        );
    }

    #[test]
    fn target_comes_from_the_frontend_as_node_or_edge() {
        let parsed: GraphTarget = serde_json::from_str(r#"{"kind":"node","row":1}"#).unwrap();
        assert_eq!(parsed, node(1));

        let parsed: GraphTarget =
            serde_json::from_str(r#"{"kind":"edge","up":1,"down":2,"style":"dotted"}"#).unwrap();
        assert_eq!(parsed, edge(1, 2, EdgeStyle::Dotted));
    }

    #[test]
    fn click_on_a_dotted_edge_expands_it() {
        let graph = chain_between_merge_and_fork(5);
        let before = graph.dump();
        let mut view = view_of(graph, &[]);
        collapse_all(&mut view);

        // Commit 5 is row 2: the dotted edge leaves from 1.
        assert!(toggle(&mut view, edge(1, 2, EdgeStyle::Dotted)));

        assert_eq!(dump(&view), before);
    }

    #[test]
    fn click_on_the_lower_dotted_edge_does_not_touch_the_upper_one() {
        let mut view = view_of(chain_between_merge_and_fork(5), &[3]);
        collapse_all(&mut view);
        assert_eq!(
            dump(&view),
            "0(1, 8)\n1(3.dot)\n3(5.dot)\n5(7)\n6(7)\n7()\n8()"
        );

        assert!(toggle(&mut view, edge(2, 3, EdgeStyle::Dotted)));

        assert_eq!(
            dump(&view),
            "0(1, 8)\n1(3.dot)\n3(4)\n4(5)\n5(7)\n6(7)\n7()\n8()"
        );
    }

    #[test]
    fn click_on_a_normal_edge_collapses_its_fragment() {
        let mut view = view_of(chain_between_merge_and_fork(5), &[]);

        assert!(toggle(&mut view, edge(2, 3, EdgeStyle::Usual)));

        assert_eq!(dump(&view), "0(1, 8)\n1(5.dot)\n5(7)\n6(7)\n7()\n8()");
    }

    #[test]
    fn the_real_edge_next_to_the_dotted_one_does_not_expand() {
        let mut view = view_of(LinearGraph::parse("0(2, 1)\n1(2)\n2()"), &[]);
        collapse_all(&mut view);
        assert_eq!(dump(&view), "0(2, 2.dot)\n2()");

        toggle(&mut view, edge(0, 1, EdgeStyle::Dotted));

        assert_eq!(view.graph.len(), 3);
    }

    #[test]
    fn click_on_something_no_longer_there_does_nothing() {
        let graph = chain_between_merge_and_fork(5);
        let before = graph.dump();
        let mut view = view_of(graph, &[]);

        assert!(!toggle(&mut view, edge(1, 4, EdgeStyle::Dotted)));
        assert!(!toggle(&mut view, edge(99, 100, EdgeStyle::Usual)));

        assert_eq!(dump(&view), before);
    }

    #[test]
    fn a_details_filter_does_not_support_clicks_on_edges() {
        let parents: [&[u32]; 3] = [&[1], &[2], &[]];
        let graph = LinearGraph::from_parents(parents.into_iter());
        let mut view = view_of(graph, &[]);
        view.collapse = Collapse::Filtered;

        assert!(!toggle(&mut view, edge(0, 1, EdgeStyle::Usual)));
        assert_eq!(hover(&view, edge(0, 1, EdgeStyle::Usual)), None);
    }

    #[test]
    fn hover_marks_the_whole_fragment_with_its_ends() {
        let view = view_of(chain_between_merge_and_fork(5), &[]);

        assert_eq!(hover(&view, node(3)), Some(vec![1, 2, 3, 4, 5]));
        assert_eq!(
            hover(&view, edge(2, 3, EdgeStyle::Usual)),
            Some(vec![1, 2, 3, 4, 5])
        );
    }

    #[test]
    fn hover_collapses_nothing() {
        let graph = chain_between_merge_and_fork(5);
        let before = graph.dump();
        let view = view_of(graph, &[]);

        hover(&view, node(3));

        assert_eq!(dump(&view), before);
    }

    #[test]
    fn hover_on_a_dotted_edge_marks_its_two_ends() {
        let mut view = view_of(chain_between_merge_and_fork(5), &[]);
        collapse_all(&mut view);

        assert_eq!(
            hover(&view, edge(1, 2, EdgeStyle::Dotted)),
            Some(vec![1, 2])
        );
        // A node with a dotted edge, the same.
        assert_eq!(hover(&view, node(2)), Some(vec![1, 2]));
    }

    #[test]
    fn hover_outside_a_fragment_marks_nothing() {
        let view = view_of(LinearGraph::parse("0(1, 2)\n1()\n2()"), &[]);

        assert_eq!(hover(&view, node(2)), None);
        assert_eq!(hover(&view, edge(0, 1, EdgeStyle::Usual)), None);
    }

    #[test]
    fn hover_over_something_no_longer_there_marks_nothing() {
        let view = view_of(chain_between_merge_and_fork(5), &[]);

        assert_eq!(hover(&view, node(99)), None);
        assert_eq!(hover(&view, edge(1, 4, EdgeStyle::Dotted)), None);
    }

    #[test]
    fn hover_bounds_what_it_walks_in_a_long_chain() {
        // `getPartLongFragment`: 500 rows on each side, and the cut is checked after each step.
        let view = view_of(chain_between_merge_and_fork(2000), &[]);

        let rows = hover(&view, node(1000)).unwrap();

        assert_eq!(rows.first(), Some(&499));
        assert_eq!(rows.last(), Some(&1502));
    }

    #[test]
    fn click_walks_the_whole_chain_even_beyond_500() {
        let mut view = view_of(chain_between_merge_and_fork(2000), &[]);

        assert!(toggle(&mut view, node(1000)));

        assert_eq!(view.graph.len(), 6);
    }

    #[test]
    fn hover_in_linear_with_branch_filter_gives_view_rows() {
        // In BEK order head 2 goes first: the merge is row 1 and its fragment, 1, 2 and 3. The filter
        // hides row 2.
        let graph = LinearGraph::parse("0(3, 1)\n1(4)\n2(4)\n3(4)\n4()");
        let mut view = view_with_branches(graph, &[], SortType::Linear);
        let base = view.delegate.clone();
        let Collapse::Merges(controller, _) = view.collapse.clone() else {
            panic!("merges are collapsed in Linear");
        };
        let mut controller = controller;
        controller.expand_all();
        let expanded = controller.compile(&base);
        let mut shown = CollapsedGraph::matching(&expanded, vec![true, true, false, true, true]);
        let graph = shown.compile(&expanded);
        view.collapse = Collapse::Merges(controller, Some(shown));
        view.set_graph(Arc::new(graph));

        let rows = hover(&view, node(1)).unwrap();

        // Row 2 is not shown, and 3 is now 2.
        assert_eq!(rows, [1, 2]);
    }

    #[test]
    fn collapsing_in_standard_uses_the_rows_of_that_order() {
        let graph = LinearGraph::parse("0(1, 2)\n1(3)\n2(4)\n3(5)\n4(5)\n5()");
        let mut view = view_with_branches(graph, &[1], SortType::Standard);
        assert_eq!(view.graph.rows, [0, 2, 4, 1, 3, 5]);

        collapse_all(&mut view);

        // Branch 1 stays in view and the others form fragments in the new order.
        assert!(view.graph.rows.contains(&1));
    }

    #[test]
    fn collapsing_updates_what_is_printed() {
        let mut view = view_of(chain_between_merge_and_fork(5), &[]);

        collapse_all(&mut view);

        let printed = crate::log::print::elements_in_rows(&view, 0, view.graph.len());
        assert_eq!(printed.len(), 6);
        assert!(printed[1]
            .edges
            .iter()
            .any(|edge| edge.style == EdgeStyle::Dotted));
    }

    #[test]
    fn collapse_all_in_linear_collapses_merges() {
        let graph = LinearGraph::parse("0(2, 1)\n1(3)\n2(3)\n3()");
        let mut view = view_with_branches(graph, &[], SortType::Linear);
        let collapsed = dump(&view);

        expand_all(&mut view);
        assert!(!view.spec.collapsed);
        assert_eq!(view.graph.len(), 4);
        assert_eq!(view.graph.downs.of(0).len(), 2);

        collapse_all(&mut view);
        assert_eq!(dump(&view), collapsed);
        assert!(view.spec.collapsed);
    }

    #[test]
    fn click_in_linear_expands_and_collapses() {
        let graph = LinearGraph::parse("0(2, 1)\n1(3)\n2(3)\n3()");
        let mut view = view_with_branches(graph, &[], SortType::Linear);
        assert_eq!(view.graph.downs.of(0).len(), 1);

        assert!(toggle(&mut view, node(1)));
        assert_eq!(view.graph.downs.of(0).len(), 2);

        assert!(toggle(&mut view, node(0)));
        assert_eq!(view.graph.downs.of(0).len(), 1);
    }

    #[test]
    fn storage_keeps_each_edge_at_both_ends_without_repeating() {
        let edge = |up: u32, down: u32| GraphEdge {
            up,
            down,
            style: EdgeStyle::Dotted,
        };
        let mut storage = EdgeStorage::default();
        storage.create(edge(1, 5));
        storage.create(edge(1, 5));
        storage.create(edge(0, 1));

        assert_eq!(storage.adjacent(1), [edge(1, 5), edge(0, 1)]);
        assert_eq!(storage.adjacent(5), [edge(1, 5)]);
        assert!(storage.has_edge(1, 5));

        storage.remove(edge(1, 5));

        assert_eq!(storage.adjacent(1), [edge(0, 1)]);
        assert!(storage.adjacent(5).is_empty());
        assert!(!storage.has_edge(1, 5));
    }

    // `FragmentGeneratorTest.MiddleNodesTest`.
    fn middle(graph: &str, up: u32, down: u32) -> Vec<u32> {
        let graph = LinearGraph::parse(graph);
        middle_nodes(&graph, up, down, false).into_iter().collect()
    }

    const SIMPLE: &str = "0(1, 2)\n1(2)\n2()";
    const TWO_BRANCH: &str = "0(2)\n1(2)\n2()";
    const DOWN_TREE: &str = "0(1, 2)\n1(3, 4)\n2(4, 5)\n3()\n4()\n5()";
    const UP_TREE: &str = "0(3)\n1(3, 4)\n2(4)\n3(5)\n4(5)\n5()";
    const DIFFICULT: &str = "0(1, 2)\n1(3, 4)\n2(6)\n3(5)\n4(6)\n5(7, 8)\n6(7, 8)\n7()\n8()";

    #[test]
    fn same_as_intellij_middle_nodes() {
        assert_eq!(middle(SIMPLE, 0, 2), [0, 1, 2]);
        assert!(middle(TWO_BRANCH, 0, 1).is_empty());
        assert_eq!(middle(DOWN_TREE, 0, 4), [0, 1, 2, 4]);
        assert_eq!(middle(UP_TREE, 1, 5), [1, 3, 4, 5]);
        assert_eq!(middle(DIFFICULT, 1, 7), [1, 3, 4, 5, 6, 7]);
        assert_eq!(middle(DIFFICULT, 0, 5), [0, 1, 3, 5]);
        assert_eq!(middle(DIFFICULT, 1, 8), [1, 3, 4, 5, 6, 8]);
    }

    #[test]
    fn strict_middle_nodes_remove_the_ends() {
        let graph = LinearGraph::parse(SIMPLE);
        let nodes: Vec<u32> = middle_nodes(&graph, 0, 2, true).into_iter().collect();
        assert_eq!(nodes, [1]);
    }
}
