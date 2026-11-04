use std::cmp::Ordering;
use std::collections::BTreeSet;

use serde::Serialize;

use crate::log::graph::{EdgeStyle, GraphEdge, LinearGraph};
use crate::log::view::{EdgeLimits, View};

const CHECKPOINT_ROWS: u32 = 1000;
// From `PrintElementGeneratorImpl` and `GraphCommitCellRenderer`.
const SAMPLE_SIZE: u32 = 20000;
const K: f64 = 0.1;
const MAX_GRAPH_WIDTH: u32 = 6;

// Segment of an edge within a row: it goes from the center of the row towards the one above or
// the one below, from column `x` to the `other_x` it has in that other row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortEdge {
    pub x: u16,
    pub other_x: u16,
    pub up: bool,
    pub color: i32,
    pub style: EdgeStyle,
    // The whole edge, which is what the mouse points at.
    pub up_row: u32,
    pub down_row: u32,
}

// Arrowhead of a segment. `terminal` is the one that cuts a hidden edge: it has no `ShortEdge`
// and carries its own short stroke. Without `terminal` it goes on the segment that reaches
// `other_x`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Arrow {
    pub x: u16,
    pub other_x: u16,
    pub up: bool,
    pub color: i32,
    pub target_row: u32,
    pub terminal: bool,
    pub up_row: u32,
    pub down_row: u32,
}

// `width` is in half columns: the width of `GraphCommitCellRenderer.getGraphWidth` can end in
// half a column because of the diagonals.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrintRow {
    pub node_x: u16,
    pub node_color: i32,
    pub width: u16,
    pub edges: Box<[ShortEdge]>,
    pub arrows: Box<[Arrow]>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    Up,
    Down,
}

#[derive(Clone, Copy)]
enum Element {
    Node(u32),
    Edge(GraphEdge),
}

type ActiveEdges = BTreeSet<GraphEdge>;

// The edges that pass through the row, not counting those that are born or die in it, every
// 1,000 rows. Storing them per row costs quadratic memory with long edges: `elements_in_rows`
// sweeps from the previous checkpoint, like `EdgesInRowGenerator`.
pub fn checkpoints(graph: &LinearGraph) -> Vec<Box<[GraphEdge]>> {
    let mut result = Vec::new();
    let mut active = ActiveEdges::new();
    for row in 0..graph.len() {
        if row % CHECKPOINT_ROWS == 0 {
            result.push(active.iter().copied().collect());
        }
        step(graph, row, &mut active);
    }
    result
}

// Moves `active` from row `row` to the next one.
fn step(graph: &LinearGraph, row: u32, active: &mut ActiveEdges) {
    active.extend(graph.downs.of(row));
    if row + 1 < graph.len() {
        for edge in graph.ups.of(row + 1) {
            active.remove(edge);
        }
    }
}

fn active_at(view: &View, row: u32) -> ActiveEdges {
    let checkpoint = row / CHECKPOINT_ROWS;
    let mut active: ActiveEdges = view.checkpoints[checkpoint as usize]
        .iter()
        .copied()
        .collect();
    for passed in checkpoint * CHECKPOINT_ROWS..row {
        step(&view.graph, passed, &mut active);
    }
    active
}

fn arrow_direction(limits: &EdgeLimits, edge: &GraphEdge, row: u32) -> Option<Direction> {
    let size = edge.down - edge.up;
    let up_offset = row - edge.up;
    let down_offset = edge.down - row;
    if size >= limits.long_edge {
        if up_offset == limits.visible_part {
            return Some(Direction::Down);
        }
        if down_offset == limits.visible_part {
            return Some(Direction::Up);
        }
    }
    if size >= limits.arrow {
        if up_offset == 1 {
            return Some(Direction::Down);
        }
        if down_offset == 1 {
            return Some(Direction::Up);
        }
    }
    None
}

fn is_visible(limits: &EdgeLimits, edge: &GraphEdge, row: u32) -> bool {
    edge.down - edge.up < limits.long_edge
        || (row - edge.up).min(edge.down - row) <= limits.visible_part
}

// `PrintElementGeneratorImpl.calculateRecommendedWidth`: weighted mean plus deviation of the
// edges per row over the first rows, with more weight on the upper ones.
pub fn recommended_width(graph: &LinearGraph, limits: &EdgeLimits) -> u32 {
    let nodes = graph.len();
    if nodes < 2 {
        return nodes;
    }
    let sample = nodes.min(SAMPLE_SIZE);
    let size = f64::from(sample);
    let mut sum = 0.0;
    let mut sum_squares = 0.0;
    let mut edges_count = 0;
    let mut current = ActiveEdges::new();
    for row in 0..sample {
        for edge in graph.ups.of(row) {
            current.remove(edge);
        }
        current.extend(graph.downs.of(row));

        let mut up_arrows = 0;
        let mut down_arrows = 0;
        let mut new_edges_count = 0;
        for edge in &current {
            if is_visible(limits, edge, row) {
                new_edges_count += 1;
            } else {
                match arrow_direction(limits, edge, row) {
                    Some(Direction::Down) => down_arrows += 1,
                    Some(Direction::Up) => up_arrows += 1,
                    None => {}
                }
            }
        }

        let width = f64::from((edges_count + up_arrows).max(new_edges_count + down_arrows));
        let weight = 2.0 / (size * (K + 1.0)) * (1.0 + (K - 1.0) * f64::from(row) / (size - 1.0));
        sum += width * weight;
        sum_squares += width * width * weight;
        edges_count = new_edges_count;
    }
    let deviation = (sum_squares - sum * sum).sqrt();
    // Like `Math.round`, and a root of a negative number caused by rounding gives 0.
    (sum + deviation + 0.5).floor() as u32
}

// `GraphElementComparatorByLayoutIndex`.
fn compare_edge_to_node(view: &View, edge: &GraphEdge, node: u32) -> Ordering {
    view.layout_index(edge.up)
        .max(view.layout_index(edge.down))
        .cmp(&view.layout_index(node))
        .then(edge.up.cmp(&node))
}

fn compare(view: &View, first: &Element, second: &Element) -> Ordering {
    match (first, second) {
        (Element::Edge(a), Element::Edge(b)) if a.up == b.up => view
            .layout_index(a.down)
            .cmp(&view.layout_index(b.down))
            .then(a.down.cmp(&b.down)),
        (Element::Edge(a), Element::Edge(b)) if a.up < b.up => compare_edge_to_node(view, a, b.up),
        (Element::Edge(a), Element::Edge(b)) => compare_edge_to_node(view, b, a.up).reverse(),
        (Element::Edge(edge), Element::Node(node)) => compare_edge_to_node(view, edge, *node),
        (Element::Node(node), Element::Edge(edge)) => {
            compare_edge_to_node(view, edge, *node).reverse()
        }
        (Element::Node(_), Element::Node(_)) => Ordering::Equal,
    }
}

// This order is not transitive in every case: an edge is compared with another by the index of
// the row where the other is born, not by that of its two ends. `sort_by` can abort with an
// order like that, and with `panic = "abort"` it would take the application down.
fn merge_sort(items: &mut Vec<Element>, view: &View) {
    let mut sorted = items.clone();
    let length = items.len();
    let mut size = 1;
    while size < length {
        for start in (0..length).step_by(2 * size) {
            let middle = (start + size).min(length);
            let end = (start + 2 * size).min(length);
            let (mut left, mut right) = (start, middle);
            for slot in &mut sorted[start..end] {
                let take_left = right == end
                    || (left < middle && compare(view, &items[left], &items[right]).is_le());
                let from = if take_left { &mut left } else { &mut right };
                *slot = items[*from];
                *from += 1;
            }
        }
        std::mem::swap(items, &mut sorted);
        size *= 2;
    }
}

// What is visible of a row, already in columns. `edge_positions` is ordered by edge.
struct RowElements {
    row: u32,
    node_x: u16,
    sorted: Vec<Element>,
    edge_positions: Vec<(GraphEdge, u16)>,
}

impl RowElements {
    fn new(view: &View, row: u32, active: &ActiveEdges) -> Self {
        let mut sorted = vec![Element::Node(row)];
        sorted.extend(
            active
                .iter()
                .filter(|edge| is_visible(&view.limits, edge, row))
                .map(|edge| Element::Edge(*edge)),
        );
        merge_sort(&mut sorted, view);
        let mut node_x = 0;
        let mut edge_positions = Vec::with_capacity(sorted.len());
        for (position, element) in sorted.iter().enumerate() {
            match element {
                Element::Node(_) => node_x = position as u16,
                Element::Edge(edge) => edge_positions.push((*edge, position as u16)),
            }
        }
        edge_positions.sort_unstable_by_key(|(edge, _)| *edge);
        Self {
            row,
            node_x,
            sorted,
            edge_positions,
        }
    }

    // The column where `edge` continues in this row: its own if it passes through it, or the
    // node's if this is its end `end`.
    fn position_of(&self, edge: &GraphEdge, end: u32) -> Option<u16> {
        match self
            .edge_positions
            .binary_search_by_key(edge, |(edge, _)| *edge)
        {
            Ok(found) => Some(self.edge_positions[found].1),
            Err(_) => (self.row == end).then_some(self.node_x),
        }
    }
}

struct RowPrinter<'a> {
    view: &'a View,
    edges: Vec<ShortEdge>,
    arrows: Vec<Arrow>,
    half_columns: u16,
}

impl RowPrinter<'_> {
    // `getGraphWidth`: counts the segment's column and, if it goes to the right, half of the
    // diagonal.
    fn widen(&mut self, x: u16, other_x: u16) {
        self.half_columns = self.half_columns.max(2 * x).max(x + other_x);
    }

    fn arrow(&mut self, edge: &GraphEdge, x: u16, other_x: u16, up: bool, terminal: bool) {
        self.arrows.push(Arrow {
            x,
            other_x,
            up,
            color: self.view.edge_color(edge),
            target_row: if up { edge.up } else { edge.down },
            terminal,
            up_row: edge.up,
            down_row: edge.down,
        });
        self.widen(x, other_x);
    }

    fn short_edge(&mut self, edge: &GraphEdge, x: u16, other_x: u16, up: bool) {
        self.edges.push(ShortEdge {
            x,
            other_x,
            up,
            color: self.view.edge_color(edge),
            style: edge.style,
            up_row: edge.up,
            down_row: edge.down,
        });
        self.widen(x, other_x);
    }

    fn side(
        &mut self,
        edge: &GraphEdge,
        x: u16,
        other_x: Option<u16>,
        up: bool,
        arrow: bool,
        node: bool,
    ) {
        match other_x {
            Some(other_x) => {
                self.short_edge(edge, x, other_x, up);
                if arrow {
                    self.arrow(edge, x, other_x, up, false);
                }
            }
            None if arrow && !node => self.arrow(edge, x, x, up, true),
            None => {}
        }
    }
}

// `PrintElementGeneratorImpl.collectElements`.
fn print_row(
    view: &View,
    current: &RowElements,
    above: Option<&RowElements>,
    below: Option<&RowElements>,
) -> PrintRow {
    let row = current.row;
    let up_position = |edge: &GraphEdge| above?.position_of(edge, edge.up);
    let down_position = |edge: &GraphEdge| below?.position_of(edge, edge.down);
    let mut printer = RowPrinter {
        view,
        edges: Vec::new(),
        arrows: Vec::new(),
        half_columns: 2 * current.node_x,
    };
    let mut print_edge = |edge: &GraphEdge, x: u16, node: bool| {
        let arrow = arrow_direction(&view.limits, edge, row);
        let (down, up) = (down_position(edge), up_position(edge));
        printer.side(edge, x, down, false, arrow == Some(Direction::Down), node);
        printer.side(edge, x, up, true, arrow == Some(Direction::Up), node);
    };
    for (position, element) in current.sorted.iter().enumerate() {
        let position = position as u16;
        match element {
            Element::Node(_) => {
                let adjacent = view.graph.ups.of(row).iter();
                for edge in adjacent.chain(view.graph.downs.of(row)) {
                    print_edge(edge, position, true);
                }
            }
            Element::Edge(edge) => print_edge(edge, position, false),
        }
    }
    let recommended = view.recommended_width.min(MAX_GRAPH_WIDTH) as u16;
    PrintRow {
        node_x: current.node_x,
        node_color: view.node_color(row),
        width: (printer.half_columns + 2).max(2 * recommended),
        edges: printer.edges.into(),
        arrows: printer.arrows.into(),
    }
}

pub fn elements_in_row(view: &View, row: u32) -> PrintRow {
    elements_in_rows(view, row, row + 1).remove(0)
}

// Each row needs to know where its edges continue in the row above and the row below.
pub fn elements_in_rows(view: &View, start: u32, end: u32) -> Vec<PrintRow> {
    if start >= end {
        return Vec::new();
    }
    let first = start.saturating_sub(1);
    let stop = (end + 1).min(view.graph.len());
    let mut active = active_at(view, first);
    let mut window = Vec::with_capacity((stop - first) as usize);
    for row in first..stop {
        window.push(RowElements::new(view, row, &active));
        step(&view.graph, row, &mut active);
    }
    (start..end)
        .map(|row| {
            let index = (row - first) as usize;
            let above = index.checked_sub(1).map(|previous| &window[previous]);
            print_row(view, &window[index], above, window.get(index + 1))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::store;
    use crate::log::view::ViewSpec;
    use crate::test_support::{Random, TempRepo};
    use tempfile::TempDir;

    // Without checkpoints: the edges come from walking all those of the graph.
    fn direct_row(view: &View, row: u32) -> PrintRow {
        let elements = |row: u32| {
            let active: ActiveEdges = (0..view.graph.len())
                .flat_map(|up| view.graph.downs.of(up))
                .filter(|edge| edge.up < row && row < edge.down)
                .copied()
                .collect();
            RowElements::new(view, row, &active)
        };
        let above = row.checked_sub(1).map(elements);
        let below = (row + 1 < view.graph.len()).then(|| elements(row + 1));
        print_row(view, &elements(row), above.as_ref(), below.as_ref())
    }

    fn view_from(graph: LinearGraph, long_edges: bool) -> View {
        let heads = graph.heads();
        let colors = vec![0; heads.len()];
        let spec = ViewSpec {
            long_edges,
            ..ViewSpec::default()
        };
        View::assemble(graph, &heads, colors, &spec)
    }

    fn view(parents: &[&[u32]]) -> View {
        view_from(LinearGraph::from_parents(parents.iter().copied()), false)
    }

    // Commit 0 with two parents: 1, which follows a chain up to `length`, and `length` itself.
    fn with_long_edge(length: u32, long_edges: bool) -> View {
        let mut parents: Vec<Vec<u32>> = vec![vec![length, 1]];
        parents.extend((1..length).map(|row| vec![row + 1]));
        parents.push(Vec::new());
        let parents: Vec<&[u32]> = parents.iter().map(Vec::as_slice).collect();
        view_from(
            LinearGraph::from_parents(parents.iter().copied()),
            long_edges,
        )
    }

    fn short_edge(row: &PrintRow, x: u16, up: bool) -> Vec<u16> {
        row.edges
            .iter()
            .filter(|edge| edge.x == x && edge.up == up)
            .map(|edge| edge.other_x)
            .collect()
    }

    fn arrow_rows(view: &View) -> Vec<(u32, bool, bool, u32)> {
        let mut arrows = Vec::new();
        for row in 0..view.graph.len() {
            for arrow in elements_in_row(view, row).arrows.iter() {
                arrows.push((row, arrow.up, arrow.terminal, arrow.target_row));
            }
        }
        arrows
    }

    #[test]
    fn straight_line() {
        let view = view(&[&[1], &[2], &[]]);

        let rows = elements_in_rows(&view, 0, 3);

        assert!(rows.iter().all(|row| row.node_x == 0));
        assert_eq!(short_edge(&rows[0], 0, false), [0]);
        assert!(short_edge(&rows[0], 0, true).is_empty());
        assert_eq!(short_edge(&rows[1], 0, true), [0]);
        assert_eq!(short_edge(&rows[1], 0, false), [0]);
        assert_eq!(short_edge(&rows[2], 0, true), [0]);
        assert!(short_edge(&rows[2], 0, false).is_empty());
    }

    #[test]
    fn column_is_the_rank_in_the_row() {
        // In row 2, (0,3) and (1,3) pass through, ending at 3. Without them the branch of row 2
        // does not continue in its column: it goes down from 2 to 1.
        let view = view(&[&[3, 1], &[3], &[4], &[5], &[5], &[]]);

        let rows = elements_in_rows(&view, 0, 6);

        assert_eq!(rows[2].node_x, 2);
        assert_eq!(short_edge(&rows[2], 2, false), [1]);
        assert_eq!(rows[3].node_x, 0);
        assert_eq!(short_edge(&rows[3], 1, true), [2]);
        assert_eq!(short_edge(&rows[3], 1, false), [1]);
        assert_eq!(rows[4].node_x, 1);
    }

    #[test]
    fn node_ordered_among_edges_by_layout_index() {
        // (0,2) belongs to chain 1 and row 1 to chain 2, so the node goes to the right.
        let view = view(&[&[2, 1], &[2], &[]]);
        assert_eq!(elements_in_row(&view, 1).node_x, 1);
    }

    #[test]
    fn edge_to_next_row_is_diagonal_center_to_center() {
        let view = view(&[&[2, 1], &[2], &[]]);

        let rows = elements_in_rows(&view, 0, 2);

        assert_eq!(short_edge(&rows[0], 0, false), [0, 1]);
        assert_eq!(short_edge(&rows[1], 1, true), [0]);
    }

    #[test]
    fn edge_of_30_rows_is_hidden() {
        let view = with_long_edge(30, false);

        assert_eq!(
            arrow_rows(&view),
            [(1, false, true, 30), (29, true, true, 0)]
        );
        // In between it no longer takes a column: the node goes back to the first one.
        assert_eq!(elements_in_row(&view, 1).node_x, 1);
        for row in 2..29 {
            assert_eq!(elements_in_row(&view, row).node_x, 0, "row {row}");
        }
        assert_eq!(elements_in_row(&view, 29).node_x, 1);
        // Row 1 joins the edge with its parent upwards, but not downwards.
        let row = elements_in_row(&view, 1);
        assert_eq!(short_edge(&row, 0, true), [0]);
        assert!(short_edge(&row, 0, false).is_empty());
    }

    #[test]
    fn edge_of_29_rows_is_drawn_whole() {
        let view = with_long_edge(29, false);

        assert!(arrow_rows(&view).is_empty());
        for row in 1..29 {
            let printed = elements_in_row(&view, row);
            assert_eq!(printed.node_x, 1, "row {row}");
            assert_eq!(short_edge(&printed, 0, true), [0], "row {row}");
            assert_eq!(short_edge(&printed, 0, false), [0], "row {row}");
        }
    }

    #[test]
    fn with_long_edges_edges_of_1000_keep_250_per_end() {
        let view = with_long_edge(1000, true);

        for row in 1..1000 {
            let expected = u16::from(row <= 250 || row >= 750);
            assert_eq!(elements_in_row(&view, row).node_x, expected, "row {row}");
        }
        // At 1 row the arrow goes on the segment; at 250, it cuts the edge.
        assert_eq!(
            arrow_rows(&view),
            [
                (1, false, false, 1000),
                (250, false, true, 1000),
                (750, true, true, 0),
                (999, true, false, 0),
            ]
        );
    }

    #[test]
    fn with_long_edges_edges_of_30_are_drawn_whole_with_arrow() {
        let view = with_long_edge(30, true);

        assert_eq!(
            arrow_rows(&view),
            [(1, false, false, 30), (29, true, false, 0)]
        );
        for row in 1..30 {
            assert_eq!(elements_in_row(&view, row).node_x, 1, "row {row}");
        }
    }

    #[test]
    fn with_long_edges_edges_of_999_rows_are_not_cut() {
        let view = with_long_edge(999, true);

        assert_eq!(
            arrow_rows(&view),
            [(1, false, false, 999), (998, true, false, 0)]
        );
    }

    #[test]
    fn arrow_leads_to_the_target_row() {
        let view = with_long_edge(40, false);

        let down = &elements_in_row(&view, 1).arrows[0];
        let up = &elements_in_row(&view, 39).arrows[0];

        assert_eq!((down.up, down.target_row), (false, 40));
        assert_eq!((up.up, up.target_row), (true, 0));
    }

    #[test]
    fn node_and_edge_colors_in_the_row() {
        let graph = LinearGraph::from_parents([&[2u32, 1][..], &[2], &[]].into_iter());
        let view = View::assemble(graph, &[0], vec![7], &ViewSpec::default());

        let rows = elements_in_rows(&view, 0, 3);

        assert_eq!(rows[0].node_color, 7);
        assert_eq!(rows[1].node_color, 2);
        let colors: Vec<_> = rows[0].edges.iter().map(|edge| edge.color).collect();
        assert_eq!(colors, [7, 2]);
    }

    #[test]
    fn width_counts_the_half_column_of_the_diagonals() {
        let mut view = view(&[&[2, 1], &[2], &[]]);
        view.recommended_width = 0;

        let rows = elements_in_rows(&view, 0, 3);

        // Row 0: node at 0 and diagonal to 1, that is 0.5 + 1 = 1.5 columns.
        assert_eq!(rows[0].width, 3);
        // Row 1: node at 1, 2 columns.
        assert_eq!(rows[1].width, 4);
    }

    #[test]
    fn width_counts_the_column_of_an_edge_that_only_goes_to_the_left() {
        let mut view = view(&[&[1], &[3, 6], &[6], &[6, 4], &[6, 5], &[6], &[]]);
        view.recommended_width = 4;

        let row = elements_in_row(&view, 5);

        // Edge (2,6) runs along column 4 and its two neighbors stay on the left.
        assert_eq!(short_edge(&row, 4, true), [3]);
        assert_eq!(short_edge(&row, 4, false), [0]);
        assert_eq!(row.width, 10);
    }

    #[test]
    fn width_reaches_the_recommended_one_up_to_6_columns() {
        let mut view = view(&[&[1], &[]]);

        view.recommended_width = 3;
        assert_eq!(elements_in_row(&view, 0).width, 6);
        view.recommended_width = 10;
        assert_eq!(elements_in_row(&view, 0).width, 12);
    }

    #[test]
    fn small_graphs_recommended_width() {
        let limits = EdgeLimits::new(false);
        let width = |parents: &[&[u32]]| {
            recommended_width(&LinearGraph::from_parents(parents.iter().copied()), &limits)
        };

        assert_eq!(width(&[]), 0);
        assert_eq!(width(&[&[]]), 1);
        assert_eq!(width(&[&[1], &[2], &[]]), 1);
        // Three branches that go down to the end, weighted mean 2.8 and deviation 0.4.
        assert_eq!(width(&[&[1, 2, 3], &[4], &[4], &[4], &[]]), 3);
    }

    #[test]
    fn long_edge_stays_in_the_checkpoints() {
        let view = with_long_edge(2500, false);
        assert_eq!(view.checkpoints.len(), 3);
        assert_eq!(view.checkpoints[0].len(), 0);
        let long = |checkpoint: &[GraphEdge]| checkpoint.iter().any(|edge| edge.down == 2500);
        assert!(long(&view.checkpoints[1]));
        assert!(long(&view.checkpoints[2]));
    }

    fn random_view(seed: u64, size: u32, long_edges: bool) -> View {
        let mut random = Random(seed);
        let mut parents: Vec<Vec<u32>> = Vec::new();
        for row in 0..size {
            let left = size - row - 1;
            let mut own = Vec::new();
            if left > 0 && random.below(40) != 0 {
                let count = 1 + random.below(10) / 8 + random.below(30) / 29;
                for _ in 0..count {
                    let reach = match random.below(20) {
                        0 => left,
                        1..=3 => left.min(200),
                        _ => left.min(6),
                    };
                    own.push(row + 1 + random.below(reach));
                }
            }
            parents.push(own);
        }
        let parents: Vec<&[u32]> = parents.iter().map(Vec::as_slice).collect();
        view_from(
            LinearGraph::from_parents(parents.iter().copied()),
            long_edges,
        )
    }

    #[test]
    fn sweep_is_the_same_with_and_without_checkpoints() {
        for long_edges in [false, true] {
            let view = random_view(20261101, 5000, long_edges);

            let swept = elements_in_rows(&view, 0, view.graph.len());

            for (row, printed) in (0..view.graph.len()).zip(&swept) {
                assert_eq!(printed, &direct_row(&view, row), "row {row}");
            }
        }
    }

    #[test]
    fn non_transitive_order_does_not_break() {
        let parents: [&[u32]; 8] = [
            &[2, 4],
            &[5],
            &[6, 3, 5],
            &[4, 7],
            &[6, 7],
            &[6, 7],
            &[7],
            &[],
        ];
        let view = view(&parents);

        let mut active = ActiveEdges::new();
        let mut out_of_order = false;
        for row in 0..view.graph.len() {
            let elements = RowElements::new(&view, row, &active).sorted;
            for (index, first) in elements.iter().enumerate() {
                out_of_order |= elements[index + 1..]
                    .iter()
                    .any(|second| compare(&view, first, second).is_gt());
            }
            step(&view.graph, row, &mut active);
        }

        assert!(out_of_order);
        assert_eq!(elements_in_rows(&view, 0, 8).len(), 8);
    }

    #[test]
    fn any_range_gives_the_same_as_the_whole_sweep() {
        let view = random_view(7, 5000, false);
        let all = elements_in_rows(&view, 0, view.graph.len());

        for (start, end) in [
            (0, 1),
            (999, 1001),
            (1000, 1050),
            (4950, 5000),
            (3333, 3334),
        ] {
            assert_eq!(
                elements_in_rows(&view, start, end),
                all[start as usize..end as usize]
            );
        }
    }

    #[test]
    fn short_edges_of_one_row_match_those_of_the_other_row() {
        let view = random_view(99, 2000, false);

        let rows = elements_in_rows(&view, 0, view.graph.len());

        // What a row draws downwards, the next one draws upwards.
        for row in 0..rows.len() - 1 {
            let mut down: Vec<_> = rows[row]
                .edges
                .iter()
                .filter(|edge| !edge.up)
                .map(|edge| (edge.x, edge.other_x, edge.color))
                .collect();
            let mut up: Vec<_> = rows[row + 1]
                .edges
                .iter()
                .filter(|edge| edge.up)
                .map(|edge| (edge.other_x, edge.x, edge.color))
                .collect();
            down.sort_unstable();
            up.sort_unstable();
            assert_eq!(down, up, "between rows {row} and {}", row + 1);
        }
    }

    #[test]
    fn difficult_graph_does_not_break() {
        // Octopus.
        let octopus = view(&[&[1, 2, 3], &[4], &[4], &[4], &[]]);
        let rows = elements_in_rows(&octopus, 0, 5);
        assert_eq!(short_edge(&rows[0], 0, false), [0, 1, 2]);

        // Multiple roots, without any edge.
        let roots = view(&[&[], &[], &[]]);
        assert!(elements_in_rows(&roots, 0, 3)
            .iter()
            .all(|row| row.node_x == 0 && row.edges.is_empty()));

        // Shallow clone.
        let origin = TempRepo::new();
        for name in ["1", "2", "3", "4"] {
            origin.commit("a.txt", name.as_bytes(), name);
        }
        let dir = TempDir::new().unwrap();
        let clone = dir.path().join("clone");
        let url = format!("file://{}", origin.path().display());
        origin.git(&["clone", "-q", "--depth", "2", &url, clone.to_str().unwrap()]);
        let store = store::load(&clone).unwrap();
        let shallow = View::build(&store, store.refs(), &ViewSpec::default());
        assert_eq!(elements_in_rows(&shallow, 0, 2).len(), 2);

        // Empty repository.
        let empty = TempRepo::new();
        let store = store::load(empty.path()).unwrap();
        let view = View::build(&store, store.refs(), &ViewSpec::default());
        assert!(view.graph.is_empty());
        assert!(elements_in_rows(&view, 0, 0).is_empty());
    }

    // The outputs of `PrintElementGeneratorTest`. The colors of that test are its own, and the
    // arrow is painted solid whatever the edge.
    fn fixture_graph(input: &str) -> LinearGraph {
        let lines: Vec<&str> = input.lines().collect();
        let row_of = |id: &str| {
            let found = lines
                .iter()
                .position(|line| line.split('_').next().unwrap() == id);
            found.unwrap() as u32
        };
        let mut edges = Vec::new();
        for (row, line) in lines.iter().enumerate() {
            let (_, parents) = line.split_once("|-").unwrap();
            for parent in parents.split_whitespace() {
                let (id, style) = parent.split_once('_').unwrap();
                edges.push(GraphEdge {
                    up: row as u32,
                    down: row_of(id),
                    style: match style {
                        "U" => EdgeStyle::Usual,
                        "D" => EdgeStyle::Dotted,
                        other => panic!("edge kind {other}"),
                    },
                });
            }
        }
        LinearGraph::from_edges((0..lines.len() as u32).collect(), &edges)
    }

    fn fixture_lines(output: &str) -> Vec<Vec<String>> {
        let mut rows: Vec<Vec<String>> = Vec::new();
        for line in output.lines() {
            let parts: Vec<&str> = line.trim().split("|-").collect();
            let kind = parts[0]
                .replace("_ARROW:SOLID", "_ARROW")
                .replace("_ARROW:DASHED", "_ARROW");
            let place = parts[1].split_once(':').unwrap().1;
            let row: usize = parts[1].split_once(':').unwrap().0.parse().unwrap();
            if rows.len() <= row {
                rows.resize(row + 1, Vec::new());
            }
            rows[row].push(format!("{kind} {place}"));
        }
        for row in &mut rows {
            row.sort();
        }
        rows
    }

    fn printed_lines(row: &PrintRow) -> Vec<String> {
        let direction = |up: bool| if up { "UP" } else { "DOWN" };
        let mut lines = vec![format!("Node {}", row.node_x)];
        for edge in row.edges.iter() {
            let arrow = row.arrows.iter().any(|arrow| {
                !arrow.terminal
                    && (arrow.x, arrow.other_x, arrow.up) == (edge.x, edge.other_x, edge.up)
            });
            let style = match (arrow, edge.style) {
                (true, _) => "_ARROW",
                (false, EdgeStyle::Usual) => ":SOLID",
                (false, _) => ":DASHED",
            };
            lines.push(format!(
                "Edge:{}{style} {}:{}",
                direction(edge.up),
                edge.x,
                edge.other_x
            ));
        }
        for arrow in row.arrows.iter().filter(|arrow| arrow.terminal) {
            lines.push(format!(
                "Edge:{}_ARROW {}:{}",
                direction(arrow.up),
                arrow.x,
                arrow.other_x
            ));
        }
        lines.sort();
        lines
    }

    fn matches_fixture(input: &str, output: &str, long_edge: u32, visible_part: u32) {
        let mut view = view_from(fixture_graph(input), false);
        view.limits = EdgeLimits {
            long_edge,
            visible_part,
            arrow: 10,
        };

        let actual: Vec<_> = elements_in_rows(&view, 0, view.graph.len())
            .iter()
            .map(printed_lines)
            .collect();

        assert_eq!(actual, fixture_lines(output));
    }

    #[test]
    fn same_as_intellij_many_nodes() {
        matches_fixture(
            include_str!("testdata/element_generator/manyNodes_in.txt"),
            include_str!("testdata/element_generator/manyNodes_out.txt"),
            7,
            2,
        );
    }

    #[test]
    fn same_as_intellij_long_edges() {
        matches_fixture(
            include_str!("testdata/element_generator/longEdges_in.txt"),
            include_str!("testdata/element_generator/longEdges_out.txt"),
            7,
            2,
        );
    }

    #[test]
    fn same_as_intellij_one_up_one_down_1() {
        matches_fixture(
            include_str!("testdata/element_generator/oneUpOneDown1_in.txt"),
            include_str!("testdata/element_generator/oneUpOneDown1_out.txt"),
            7,
            1,
        );
    }

    #[test]
    fn same_as_intellij_one_up_one_down_2() {
        matches_fixture(
            include_str!("testdata/element_generator/oneUpOneDown2_in.txt"),
            include_str!("testdata/element_generator/oneUpOneDown2_out.txt"),
            10,
            1,
        );
    }
}
