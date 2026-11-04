use std::sync::{Arc, OnceLock};

use serde::Deserialize;

use crate::log::collapse::{self, Collapse, CollapsedGraph};
use crate::log::filter::{self, LogFilter, Selection};
use crate::log::graph::{GraphEdge, LinearGraph};
use crate::log::layout;
use crate::log::linear_bek::LinearBekController;
use crate::log::print;
use crate::log::refs::{RefKind, Refs};
use crate::log::sort;
use crate::log::store::CommitStore;

// The color of a head without references, which is painted black.
const DEFAULT_COLOR: i32 = 0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SortType {
    #[default]
    Off,
    Standard,
    Linear,
}

// `collapsed` is the last button pressed. In Linear order the view is always born collapsed, like
// `LinearBekController`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ViewSpec {
    pub sort: SortType,
    pub long_edges: bool,
    pub collapsed: bool,
    pub filter: LogFilter,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewPatch {
    pub sort: Option<SortType>,
    pub long_edges: Option<bool>,
    pub collapsed: Option<bool>,
}

// Sizes of `PrintElementGeneratorImpl`: from `long_edge` on, an edge is only drawn within
// `visible_part` rows of its ends, and from `arrow` on it gets an arrow 1 row from them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EdgeLimits {
    pub long_edge: u32,
    pub visible_part: u32,
    pub arrow: u32,
}

impl EdgeLimits {
    pub fn new(long_edges: bool) -> Self {
        if long_edges {
            Self {
                long_edge: 1000,
                visible_part: 250,
                arrow: 30,
            }
        } else {
            Self {
                long_edge: 30,
                visible_part: 1,
                arrow: u32::MAX,
            }
        }
    }
}

// `String.hashCode` over UTF-16: a branch's color comes from it.
fn java_hash(text: &str) -> i32 {
    text.encode_utf16().fold(0, |hash: i32, unit| {
        hash.wrapping_mul(31).wrapping_add(i32::from(unit))
    })
}

// What does not depend on the view, as in `PermanentGraphImpl`. `layout`, `times` and `branches`
// are per commit, that is, per row of `graph`.
pub struct Permanent {
    pub(crate) graph: Arc<LinearGraph>,
    pub(crate) layout: Vec<u32>,
    // Per head, in order: its layout index, which is the first of its own, and its color.
    head_starts: Vec<u32>,
    head_colors: Vec<i32>,
    times: Vec<i64>,
    pub(crate) branches: Vec<bool>,
    bek: OnceLock<Arc<LinearGraph>>,
}

impl Permanent {
    pub(crate) fn new(
        graph: LinearGraph,
        heads: &[u32],
        head_colors: Vec<i32>,
        times: Vec<i64>,
        branches: Vec<bool>,
    ) -> Self {
        let layout = layout::layout_indexes(&graph, heads);
        Self {
            head_starts: heads.iter().map(|row| layout[*row as usize]).collect(),
            head_colors,
            times,
            branches,
            layout,
            graph: Arc::new(graph),
            bek: OnceLock::new(),
        }
    }

    pub(crate) fn from_store(store: &CommitStore, refs: &Refs) -> Self {
        let graph = LinearGraph::from_parents(store.parents());
        let (heads, colors) = layout::sorted_heads(&graph, refs)
            .into_iter()
            .map(|(row, best)| {
                let color = best.map_or(DEFAULT_COLOR, |git_ref| java_hash(&git_ref.name));
                (row, color)
            })
            .unzip::<u32, i32, Vec<_>, Vec<_>>();
        // `GitLogRecord.getCommitTime`: the commit's date, in milliseconds.
        let times = (0..store.len())
            .map(|index| store.get(index).commit_time * 1000)
            .collect();
        // `SimpleRefType.isBranch`: everything but the tags.
        let mut branches = vec![false; store.len() as usize];
        for git_ref in refs
            .all
            .iter()
            .filter(|git_ref| git_ref.kind != RefKind::Tag)
        {
            branches[git_ref.commit as usize] = true;
        }
        Self::new(graph, &heads, colors, times, branches)
    }

    fn bek_graph(&self) -> Arc<LinearGraph> {
        self.bek
            .get_or_init(|| {
                let order = sort::bek_order(&self.graph, &self.layout, &self.times);
                Arc::new(sort::reordered(&self.graph, &order))
            })
            .clone()
    }
}

#[derive(Clone)]
pub struct View {
    pub spec: ViewSpec,
    // What is drawn: `delegate` with what is collapsed.
    pub graph: Arc<LinearGraph>,
    pub(crate) permanent: Arc<Permanent>,
    // The graph in the view's order, not collapsed.
    pub(crate) delegate: Arc<LinearGraph>,
    pub(crate) collapse: Collapse,
    selection: Selection,
    pub(crate) limits: EdgeLimits,
    pub(crate) checkpoints: Vec<Box<[GraphEdge]>>,
    pub(crate) recommended_width: u32,
}

impl View {
    pub fn build(store: &CommitStore, refs: &Refs, spec: &ViewSpec) -> Self {
        Self::new(Arc::new(Permanent::from_store(store, refs)), spec)
    }

    pub fn new(permanent: Arc<Permanent>, spec: &ViewSpec) -> Self {
        Self::filtered(permanent, spec, Selection::All)
    }

    // `PermanentGraphImpl.createFilteredController`: the filter goes on top of the order, and that is
    // why it keeps it. In Linear order, on top of its collapsed merges.
    pub fn filtered(permanent: Arc<Permanent>, spec: &ViewSpec, selection: Selection) -> Self {
        let delegate = match spec.sort {
            SortType::Off => permanent.graph.clone(),
            SortType::Standard | SortType::Linear => permanent.bek_graph(),
        };
        let mut spec = spec.clone();
        let controller = (spec.sort == SortType::Linear)
            .then(|| LinearBekController::new(&delegate, &permanent.layout));
        let base = match &controller {
            Some(controller) => Arc::new(controller.compile(&delegate)),
            None => delegate.clone(),
        };
        spec.collapsed |= controller.is_some();
        let (collapse, graph) = match (&selection, controller) {
            (Selection::All, Some(controller)) => (Collapse::Merges(controller, None), base),
            (Selection::All, None) => (Collapse::branches(&delegate), base),
            (Selection::Heads(kept), controller) => {
                let mut shown = CollapsedGraph::of_commits(&base, kept);
                let graph = Arc::new(shown.compile(&base));
                let collapse = match controller {
                    Some(controller) => Collapse::Merges(controller, Some(shown)),
                    None => Collapse::Branches(shown),
                };
                (collapse, graph)
            }
            (Selection::Commits(kept), controller) => {
                spec.collapsed = controller.is_some();
                let graph = Arc::new(filter::dotted_edges(&base, kept));
                (Collapse::Filtered, graph)
            }
        };
        let limits = EdgeLimits::new(spec.long_edges);
        let mut view = Self {
            checkpoints: print::checkpoints(&graph),
            recommended_width: print::recommended_width(&graph, &limits),
            limits,
            collapse,
            delegate,
            graph,
            permanent,
            spec,
            selection,
        };
        if view.spec.sort != SortType::Linear && view.spec.collapsed {
            collapse::collapse_all(&mut view);
        }
        view
    }

    // `isActionSupported`: the collapse buttons, which a details filter disables.
    pub fn collapsible(&self) -> bool {
        !matches!(self.collapse, Collapse::Filtered)
    }

    #[cfg(test)]
    pub(crate) fn assemble(
        graph: LinearGraph,
        heads: &[u32],
        head_colors: Vec<i32>,
        spec: &ViewSpec,
    ) -> Self {
        let size = graph.rows.len();
        let permanent = Permanent::new(graph, heads, head_colors, vec![0; size], vec![false; size]);
        Self::new(Arc::new(permanent), spec)
    }

    // What `VisibleGraphImpl.updatePrintElementGenerator` does after the graph changes.
    pub(crate) fn set_graph(&mut self, graph: Arc<LinearGraph>) {
        self.checkpoints = print::checkpoints(&graph);
        self.recommended_width = print::recommended_width(&graph, &self.limits);
        self.graph = graph;
    }

    // Another order is another view: what was collapsed by hand is lost, as when asking for another
    // `VisibleGraph`.
    pub fn apply(&mut self, patch: &ViewPatch) {
        if let Some(long_edges) = patch.long_edges {
            self.spec.long_edges = long_edges;
            self.limits = EdgeLimits::new(long_edges);
            self.recommended_width = print::recommended_width(&self.graph, &self.limits);
        }
        if let Some(sort) = patch.sort.filter(|sort| *sort != self.spec.sort) {
            let spec = ViewSpec {
                sort,
                collapsed: false,
                ..self.spec.clone()
            };
            *self = Self::filtered(self.permanent.clone(), &spec, self.selection.clone());
        }
        match patch.collapsed {
            Some(true) => collapse::collapse_all(self),
            Some(false) => collapse::expand_all(self),
            None => {}
        }
    }

    pub fn layout_index(&self, row: u32) -> u32 {
        self.permanent.layout[self.graph.rows[row as usize] as usize]
    }

    // `GraphColorGetterByHead`: the head's chain takes the color of its branch, and the others that
    // of their index.
    pub fn node_color(&self, row: u32) -> i32 {
        let layout_index = self.layout_index(row);
        let permanent = &self.permanent;
        let head = permanent
            .head_starts
            .partition_point(|start| *start <= layout_index)
            .saturating_sub(1);
        if permanent.head_starts[head] == layout_index {
            permanent.head_colors[head]
        } else {
            layout_index as i32
        }
    }

    pub fn edge_color(&self, edge: &GraphEdge) -> i32 {
        if self.layout_index(edge.up) >= self.layout_index(edge.down) {
            self.node_color(edge.up)
        } else {
            self.node_color(edge.down)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::graph::EdgeStyle;
    use crate::log::store;
    use crate::test_support::TempRepo;

    const MASTER_HASH: i32 = -1081267614;

    fn edge(up: u32, down: u32) -> GraphEdge {
        GraphEdge {
            up,
            down,
            style: EdgeStyle::Usual,
        }
    }

    fn view_of(repo: &TempRepo) -> View {
        let store = store::load(repo.path()).unwrap();
        View::build(&store, store.refs(), &ViewSpec::default())
    }

    fn row_of(view: &View, repo: &TempRepo, subject: &str) -> u32 {
        let store = store::load(repo.path()).unwrap();
        (0..view.graph.len())
            .find(|row| &*store.get(view.graph.rows[*row as usize]).subject == subject)
            .unwrap()
    }

    // main: base - on main - merge, with `side` as the second parent of the merge.
    fn repo_with_merge() -> TempRepo {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "base");
        repo.git(&["branch", "-m", "master"]);
        repo.git(&["checkout", "-q", "-b", "other"]);
        repo.commit("b.txt", b"2", "side");
        repo.git(&["checkout", "-q", "master"]);
        repo.commit("c.txt", b"3", "on main");
        repo.git(&["merge", "-q", "--no-ff", "-m", "merge", "other"]);
        repo.git(&["branch", "-D", "other"]);
        repo
    }

    fn with_spec(repo: &TempRepo, spec: &ViewSpec) -> View {
        let store = store::load(repo.path()).unwrap();
        View::build(&store, store.refs(), spec)
    }

    fn sorted(sort: SortType) -> ViewSpec {
        ViewSpec {
            sort,
            ..ViewSpec::default()
        }
    }

    fn subjects(view: &View, repo: &TempRepo) -> Vec<String> {
        let store = store::load(repo.path()).unwrap();
        (0..view.graph.len())
            .map(|row| store.get(view.graph.rows[row as usize]).subject.to_string())
            .collect()
    }

    // Commits of the branch and of main interleaved by date, with the merge on top.
    fn repo_interleaved() -> TempRepo {
        let repo = TempRepo::new();
        let commit = |subject: &str, date: u32| {
            let date = format!("2026-01-0{date}T12:00:00");
            repo.commit_dated(&format!("{subject}.txt"), subject, &date);
        };
        commit("base", 1);
        repo.git(&["branch", "-m", "master"]);
        repo.git(&["checkout", "-q", "-b", "other"]);
        commit("branch 1", 2);
        repo.git(&["checkout", "-q", "master"]);
        commit("main 1", 3);
        repo.git(&["checkout", "-q", "other"]);
        commit("branch 2", 4);
        repo.git(&["checkout", "-q", "master"]);
        commit("main 2", 5);
        repo.merge_dated("other", "merge", "2026-01-06T12:00:00");
        repo.git(&["branch", "-D", "other"]);
        repo
    }

    #[test]
    fn off_keeps_gits_order() {
        let repo = repo_interleaved();
        let view = with_spec(&repo, &ViewSpec::default());
        assert_eq!(
            subjects(&view, &repo),
            ["merge", "main 2", "branch 2", "main 1", "branch 1", "base"]
        );
    }

    #[test]
    fn standard_leaves_the_branch_right_below_the_merge() {
        let repo = repo_interleaved();
        let view = with_spec(&repo, &sorted(SortType::Standard));
        assert_eq!(
            subjects(&view, &repo),
            ["merge", "branch 2", "branch 1", "main 2", "main 1", "base"]
        );
    }

    #[test]
    fn color_follows_the_commit_in_another_order() {
        let repo = repo_interleaved();
        let view = with_spec(&repo, &sorted(SortType::Standard));

        assert_eq!(view.node_color(0), MASTER_HASH);
        assert_eq!(view.node_color(1), 2);
        assert_eq!(view.node_color(2), 2);
        assert_eq!(view.node_color(3), MASTER_HASH);
    }

    #[test]
    fn linear_is_born_collapsed_with_the_rows_of_standard() {
        let repo = repo_interleaved();

        let view = with_spec(&repo, &sorted(SortType::Linear));

        assert_eq!(
            subjects(&view, &repo),
            ["merge", "branch 2", "branch 1", "main 2", "main 1", "base"]
        );
        assert!(view.spec.collapsed);
        assert_eq!(view.graph.downs.of(0).len(), 1);
        assert_eq!(view.graph.downs.of(2)[0].style, EdgeStyle::Dotted);
    }

    #[test]
    fn changing_order_does_not_sort_again() {
        let repo = repo_interleaved();
        let mut view = with_spec(&repo, &sorted(SortType::Standard));
        let first = view.delegate.clone();

        view.apply(&ViewPatch {
            sort: Some(SortType::Off),
            ..ViewPatch::default()
        });
        view.apply(&ViewPatch {
            sort: Some(SortType::Linear),
            ..ViewPatch::default()
        });

        assert!(Arc::ptr_eq(&first, &view.delegate));
    }

    #[test]
    fn changing_order_forgets_what_was_collapsed() {
        let repo = repo_interleaved();
        let mut view = with_spec(&repo, &ViewSpec::default());
        view.apply(&ViewPatch {
            collapsed: Some(true),
            ..ViewPatch::default()
        });
        assert!(view.spec.collapsed);
        assert!(view.graph.len() < 6);

        view.apply(&ViewPatch {
            sort: Some(SortType::Standard),
            ..ViewPatch::default()
        });

        assert!(!view.spec.collapsed);
        assert_eq!(view.graph.len(), 6);
    }

    #[test]
    fn order_and_collapse_in_the_same_change() {
        let repo = repo_interleaved();
        let mut view = with_spec(&repo, &ViewSpec::default());

        view.apply(&ViewPatch {
            sort: Some(SortType::Linear),
            collapsed: Some(false),
            ..ViewPatch::default()
        });

        assert!(!view.spec.collapsed);
        assert_eq!(view.graph.downs.of(0).len(), 2);
    }

    #[test]
    fn same_order_keeps_what_was_collapsed() {
        let repo = repo_interleaved();
        let mut view = with_spec(&repo, &ViewSpec::default());
        view.apply(&ViewPatch {
            collapsed: Some(true),
            ..ViewPatch::default()
        });
        let count = view.graph.len();

        view.apply(&ViewPatch {
            sort: Some(SortType::Off),
            ..ViewPatch::default()
        });

        assert_eq!(view.graph.len(), count);
    }

    #[test]
    fn long_edges_changes_the_limits_without_touching_what_was_collapsed() {
        let repo = repo_interleaved();
        let mut view = with_spec(&repo, &ViewSpec::default());
        view.apply(&ViewPatch {
            collapsed: Some(true),
            ..ViewPatch::default()
        });
        let count = view.graph.len();

        view.apply(&ViewPatch {
            long_edges: Some(true),
            ..ViewPatch::default()
        });

        assert_eq!(view.limits, EdgeLimits::new(true));
        assert!(view.spec.long_edges);
        assert_eq!(view.graph.len(), count);
    }

    #[test]
    fn commit_date_in_milliseconds() {
        let repo = repo_interleaved();
        let store = store::load(repo.path()).unwrap();

        let permanent = Permanent::from_store(&store, store.refs());

        assert_eq!(permanent.times[0], store.get(0).commit_time * 1000);
        assert!(permanent.times[0] > 1_000_000_000_000);
    }

    #[test]
    fn branches_and_head_count_as_branch_but_tags_do_not() {
        let repo = TempRepo::new();
        repo.commit("a.txt", b"1", "with tag");
        repo.git(&["tag", "v1"]);
        repo.commit("a.txt", b"2", "with nothing");
        repo.commit("a.txt", b"3", "with branch");
        repo.git(&["branch", "other"]);
        repo.commit("a.txt", b"4", "tip");
        let store = store::load(repo.path()).unwrap();

        let permanent = Permanent::from_store(&store, store.refs());

        assert_eq!(permanent.branches, [true, true, false, false]);
    }

    #[test]
    fn limits_without_long_edges() {
        assert_eq!(
            EdgeLimits::new(false),
            EdgeLimits {
                long_edge: 30,
                visible_part: 1,
                arrow: u32::MAX
            }
        );
    }

    #[test]
    fn limits_with_long_edges() {
        assert_eq!(
            EdgeLimits::new(true),
            EdgeLimits {
                long_edge: 1000,
                visible_part: 250,
                arrow: 30
            }
        );
    }

    #[test]
    fn hash_as_in_java() {
        assert_eq!(java_hash("master"), MASTER_HASH);
        assert_eq!(java_hash(""), 0);
        // Outside the basic plane each UTF-16 unit counts.
        assert_eq!(java_hash("😀"), 1772899);
    }

    #[test]
    fn color_of_the_head_chain_node_is_the_hash_of_the_name() {
        let repo = repo_with_merge();
        let view = view_of(&repo);

        for subject in ["merge", "on main", "base"] {
            assert_eq!(view.node_color(row_of(&view, &repo, subject)), MASTER_HASH);
        }
    }

    #[test]
    fn color_of_the_rest_is_the_index() {
        let repo = repo_with_merge();
        let view = view_of(&repo);

        let row = row_of(&view, &repo, "side");

        assert_eq!(view.layout_index(row), 2);
        assert_eq!(view.node_color(row), 2);
    }

    #[test]
    fn head_without_reference_is_black() {
        let parents: [&[u32]; 2] = [&[1], &[]];
        let graph = LinearGraph::from_parents(parents.into_iter());
        let view = View::assemble(graph, &[0], vec![0], &ViewSpec::default());
        assert_eq!(view.node_color(0), 0);
        assert_eq!(view.node_color(1), 0);
    }

    #[test]
    fn color_of_each_head_is_its_own() {
        let parents: [&[u32]; 4] = [&[2], &[2, 3], &[], &[]];
        let graph = LinearGraph::from_parents(parents.into_iter());
        let view = View::assemble(graph, &[1, 0], vec![9, 7], &ViewSpec::default());

        assert_eq!(view.node_color(1), 9);
        assert_eq!(view.node_color(2), 9);
        assert_eq!(view.node_color(0), 7);
        // The chain of the first head's second parent is not the head's chain.
        assert_eq!(view.node_color(3), 2);
    }

    #[test]
    fn edge_color_is_that_of_the_end_with_the_greater_index() {
        let parents: [&[u32]; 4] = [&[2, 1], &[3], &[3], &[]];
        let graph = LinearGraph::from_parents(parents.into_iter());
        let view = View::assemble(graph, &[0], vec![7], &ViewSpec::default());

        // From 0 (index 1) to 1 (index 2): the lower one rules, which is not in the head chain.
        assert_eq!(view.edge_color(&edge(0, 1)), 2);
        // From 1 (index 2) to 3 (index 1): the upper one rules.
        assert_eq!(view.edge_color(&edge(1, 3)), 2);
        // Tie: the upper one.
        assert_eq!(view.edge_color(&edge(0, 2)), 7);
    }
}
