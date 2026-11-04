use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EdgeStyle {
    Usual,
    Dotted,
    Dashed,
}

// `up` is the child's row and `down` the parent's: `up < down`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct GraphEdge {
    pub up: u32,
    pub down: u32,
    pub style: EdgeStyle,
}

// The edges of all rows go one after another in a single vector: a `Vec` per row would cost more
// than the edges themselves with 100,000 commits.
#[derive(Debug, Default)]
pub struct Adjacency {
    starts: Vec<u32>,
    edges: Vec<GraphEdge>,
}

impl Adjacency {
    fn new(row_count: usize, edges: &[GraphEdge], row_of: impl Fn(&GraphEdge) -> u32) -> Self {
        let mut starts = vec![0; row_count + 1];
        for edge in edges {
            starts[row_of(edge) as usize + 1] += 1;
        }
        for row in 0..row_count {
            starts[row + 1] += starts[row];
        }
        let mut next = starts.clone();
        let mut sorted = edges.to_vec();
        for edge in edges {
            let slot = &mut next[row_of(edge) as usize];
            sorted[*slot as usize] = *edge;
            *slot += 1;
        }
        Self {
            starts,
            edges: sorted,
        }
    }

    pub fn of(&self, row: u32) -> &[GraphEdge] {
        let row = row as usize;
        &self.edges[self.starts[row] as usize..self.starts[row + 1] as usize]
    }
}

// `rows` gives the commit of each row. `ups` and `downs` give the edges that reach a row from
// above and those that leave downwards; the latter keep the order of the parents.
#[derive(Debug)]
pub struct LinearGraph {
    pub rows: Vec<u32>,
    pub ups: Adjacency,
    pub downs: Adjacency,
}

impl LinearGraph {
    pub fn from_edges(rows: Vec<u32>, edges: &[GraphEdge]) -> Self {
        Self {
            ups: Adjacency::new(rows.len(), edges, |edge| edge.down),
            downs: Adjacency::new(rows.len(), edges, |edge| edge.up),
            rows,
        }
    }

    // One row per commit, in the order of the store. A repeated parent is a single edge, as in
    // `DuplicateParentFixer`.
    pub fn from_parents<'a>(parents: impl Iterator<Item = &'a [u32]>) -> Self {
        let mut rows = Vec::new();
        let mut edges: Vec<GraphEdge> = Vec::new();
        for (row, commit_parents) in parents.enumerate() {
            let row = row as u32;
            let first = edges.len();
            rows.push(row);
            for parent in commit_parents {
                if !edges[first..].iter().any(|edge| edge.down == *parent) {
                    edges.push(GraphEdge {
                        up: row,
                        down: *parent,
                        style: EdgeStyle::Usual,
                    });
                }
            }
        }
        Self::from_edges(rows, &edges)
    }

    pub fn len(&self) -> u32 {
        self.rows.len() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    // The rows without children.
    pub fn heads(&self) -> Vec<u32> {
        (0..self.len())
            .filter(|row| self.ups.of(*row).is_empty())
            .collect()
    }
}

// The graphs of IntelliJ's tests, for example `0(1, 2.dot)`: one commit per line.
#[cfg(test)]
impl LinearGraph {
    pub(crate) fn parse(text: &str) -> Self {
        let mut rows = Vec::new();
        let mut edges = Vec::new();
        for line in text.lines() {
            let (row, parents) = line.trim_end_matches(')').split_once('(').unwrap();
            let row: u32 = row.parse().unwrap();
            assert_eq!(row as usize, rows.len());
            rows.push(row);
            for parent in parents.split(", ").filter(|parent| !parent.is_empty()) {
                let (down, style) = match parent.strip_suffix(".dot") {
                    Some(down) => (down, EdgeStyle::Dotted),
                    None => (parent, EdgeStyle::Usual),
                };
                edges.push(GraphEdge {
                    up: row,
                    down: down.parse().unwrap(),
                    style,
                });
            }
        }
        Self::from_edges(rows, &edges)
    }

    pub(crate) fn dump(&self) -> String {
        let lines: Vec<String> = (0..self.len())
            .map(|row| {
                let parents: Vec<String> = self
                    .downs
                    .of(row)
                    .iter()
                    .map(|edge| {
                        let id = self.rows[edge.down as usize];
                        match edge.style {
                            EdgeStyle::Dotted => format!("{id}.dot"),
                            _ => id.to_string(),
                        }
                    })
                    .collect();
                format!("{}({})", self.rows[row as usize], parents.join(", "))
            })
            .collect();
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn downs(graph: &LinearGraph, row: u32) -> Vec<u32> {
        graph.downs.of(row).iter().map(|edge| edge.down).collect()
    }

    fn ups(graph: &LinearGraph, row: u32) -> Vec<u32> {
        graph.ups.of(row).iter().map(|edge| edge.up).collect()
    }

    #[test]
    fn edges_in_parent_order() {
        let parents: [&[u32]; 4] = [&[3, 1], &[3], &[3], &[]];
        let graph = LinearGraph::from_parents(parents.into_iter());

        assert_eq!(downs(&graph, 0), [3, 1]);
        assert_eq!(ups(&graph, 3), [0, 1, 2]);
        assert_eq!(ups(&graph, 1), [0]);
        assert!(downs(&graph, 3).is_empty());
    }

    #[test]
    fn repeated_parent_is_one_edge() {
        let parents: [&[u32]; 2] = [&[1, 1], &[]];
        let graph = LinearGraph::from_parents(parents.into_iter());
        assert_eq!(downs(&graph, 0), [1]);
        assert_eq!(ups(&graph, 1), [0]);
    }

    #[test]
    fn heads_are_the_rows_without_children() {
        let parents: [&[u32]; 4] = [&[2], &[3], &[3], &[]];
        let graph = LinearGraph::from_parents(parents.into_iter());
        assert_eq!(graph.heads(), [0, 1]);
    }
}
