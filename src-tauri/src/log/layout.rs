use std::cmp::Ordering;

use crate::log::graph::LinearGraph;
use crate::log::refs::{best_ref, compare_layout, GitRef, Refs};

// `GraphLayoutBuilder`: each head is walked depth-first, always through the first parent without
// an index. The index advances only when a chain ends, so that a chain of first parents shares
// its own and each other parent starts another.
pub fn layout_indexes(graph: &LinearGraph, heads: &[u32]) -> Vec<u32> {
    let mut layout = vec![0; graph.len() as usize];
    let mut current = 1;
    let mut stack = Vec::new();
    for head in heads {
        stack.push(*head);
        while let Some(row) = stack.last().copied() {
            let first_visit = layout[row as usize] == 0;
            if first_visit {
                layout[row as usize] = current;
            }
            let next = graph
                .downs
                .of(row)
                .iter()
                .find(|edge| layout[edge.down as usize] == 0);
            match next {
                Some(edge) => stack.push(edge.down),
                None => {
                    if first_visit {
                        current += 1;
                    }
                    stack.pop();
                }
            }
        }
    }
    layout
}

// The rows without children in the order of `HeadCommitsComparator`, each with its best
// reference.
pub fn sorted_heads<'a>(graph: &LinearGraph, refs: &'a Refs) -> Vec<(u32, Option<&'a GitRef>)> {
    let mut heads: Vec<_> = graph
        .heads()
        .into_iter()
        .map(|row| {
            let commit = graph.rows[row as usize];
            (row, best_ref(&refs.of_commit(commit)))
        })
        .collect();
    heads.sort_by(|(row_a, ref_a), (row_b, ref_b)| match (ref_a, ref_b) {
        (Some(a), Some(b)) => compare_layout(a, b),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => row_a.cmp(row_b),
    });
    heads
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::refs::RefKind;

    fn graph(parents: &[&[u32]]) -> LinearGraph {
        LinearGraph::from_parents(parents.iter().copied())
    }

    fn git_ref(name: &str, kind: RefKind, commit: u32) -> GitRef {
        GitRef {
            name: name.into(),
            kind,
            commit,
        }
    }

    #[test]
    fn indexes_start_at_one() {
        let graph = graph(&[&[1], &[]]);
        assert_eq!(layout_indexes(&graph, &[0]), [1, 1]);
    }

    #[test]
    fn first_parent_chain_shares_index() {
        let graph = graph(&[&[1], &[2], &[3], &[]]);
        assert_eq!(layout_indexes(&graph, &[0]), [1, 1, 1, 1]);
    }

    #[test]
    fn another_parent_starts_a_chain() {
        // Merge 0 continues through its first parent, 2; the second, 1, starts another chain.
        let graph = graph(&[&[2, 1], &[3], &[3], &[]]);
        assert_eq!(layout_indexes(&graph, &[0]), [1, 2, 1, 1]);
    }

    #[test]
    fn each_head_starts_at_the_next_index() {
        let graph = graph(&[&[2], &[2], &[]]);
        assert_eq!(layout_indexes(&graph, &[1, 0]), [2, 1, 1]);
    }

    #[test]
    fn commit_reached_earlier_by_another_path_keeps_its_index() {
        // The merge's second parent hangs off the same root as the first.
        let graph = graph(&[&[1, 2], &[3], &[3], &[]]);
        assert_eq!(layout_indexes(&graph, &[0]), [1, 1, 2, 1]);
    }

    #[test]
    fn heads_sorted_by_best_reference() {
        let graph = graph(&[&[3], &[3], &[3], &[]]);
        let refs = Refs {
            all: vec![
                git_ref("other", RefKind::Local, 0),
                git_ref("origin/main", RefKind::Remote, 2),
            ],
            ..Refs::default()
        };

        let heads = sorted_heads(&graph, &refs);

        let order: Vec<_> = heads
            .iter()
            .map(|(row, best)| (*row, best.map(|git_ref| &*git_ref.name)))
            .collect();
        // Without a reference, last.
        assert_eq!(
            order,
            [(2, Some("origin/main")), (0, Some("other")), (1, None)]
        );
    }
}
