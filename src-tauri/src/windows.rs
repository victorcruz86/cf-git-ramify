//! Window labels. The first project window is `main`, with the auxiliary windows `diff`, `merge`,
//! `viewer` and `selection-history`; later ones are `main-<n>`, `diff-<n>` and so on, with n >= 2
//! written without leading zeros. The owner of a label is the label of its project's main window.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowKind {
    Main,
    Diff,
    Merge,
    Viewer,
    SelectionHistory,
}

const KINDS: [WindowKind; 5] = [
    WindowKind::Main,
    WindowKind::Diff,
    WindowKind::Merge,
    WindowKind::Viewer,
    WindowKind::SelectionHistory,
];

impl WindowKind {
    fn prefix(self) -> &'static str {
        match self {
            WindowKind::Main => "main",
            WindowKind::Diff => "diff",
            WindowKind::Merge => "merge",
            WindowKind::Viewer => "viewer",
            WindowKind::SelectionHistory => "selection-history",
        }
    }
}

/// Splits a label into its kind and its number (`None` for the first project window's labels).
fn parse(label: &str) -> Option<(WindowKind, Option<u32>)> {
    for kind in KINDS {
        let prefix = kind.prefix();
        let Some(rest) = label.strip_prefix(prefix) else {
            continue;
        };
        if rest.is_empty() {
            return Some((kind, None));
        }
        let Some(digits) = rest.strip_prefix('-') else {
            continue;
        };
        if digits.is_empty()
            || digits.starts_with('0')
            || !digits.bytes().all(|b| b.is_ascii_digit())
        {
            continue;
        }
        return match digits.parse::<u32>() {
            Ok(n) if n >= 2 => Some((kind, Some(n))),
            _ => None,
        };
    }
    None
}

pub fn kind_of(label: &str) -> Option<WindowKind> {
    parse(label).map(|(kind, _)| kind)
}

/// The main window's label of the project window `label` belongs to.
pub fn owner_of(label: &str) -> Option<String> {
    parse(label).map(|(_, n)| label_for_number(WindowKind::Main, n))
}

fn label_for_number(kind: WindowKind, n: Option<u32>) -> String {
    match n {
        None => kind.prefix().to_string(),
        Some(n) => format!("{}-{n}", kind.prefix()),
    }
}

/// The label of `kind` in the project window owned by `owner`. A label that is not an owner
/// is treated as the first project window's in a release build; a debug build stops, because
/// whoever asks has lost the owner.
pub fn label_for(kind: WindowKind, owner: &str) -> String {
    debug_assert!(
        matches!(parse(owner), Some((WindowKind::Main, _))),
        "{owner:?} is not the label of a main window"
    );
    match parse(owner) {
        Some((WindowKind::Main, n)) => label_for_number(kind, n),
        _ => kind.prefix().to_string(),
    }
}

/// The five labels of one project window: main, diff, merge, viewer, selection history.
pub fn owner_group(owner: &str) -> [String; 5] {
    KINDS.map(|kind| label_for(kind, owner))
}

/// `main` first, then `main-2`, `main-3`...: a number is never reused in the process, even
/// after its window is gone. `counter` is the last number handed out, `main` being 1 whether
/// or not this function gave it (the first window comes from the app's configuration). Each
/// step on `counter` is one atomic operation, so two calls at once never get the same label.
pub fn next_main_label(taken: &HashSet<String>, counter: &AtomicU32) -> String {
    if !taken.contains("main")
        && counter
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    {
        return "main".to_string();
    }
    loop {
        let previous = counter
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| Some(n.max(1) + 1))
            .expect("the update always gives a value");
        let label = format!("main-{}", previous.max(1) + 1);
        if !taken.contains(&label) {
            return label;
        }
    }
}

/// What each main window's page opens when it starts (`initial_target`), by its label. A window
/// made by the open flow has its repository stored here; only the first `main` reads the command
/// line, and a stored target wins over it.
#[derive(Debug, Default)]
pub struct InitialTargets {
    stored: HashMap<String, String>,
    asked: HashSet<String>,
}

impl InitialTargets {
    pub fn store(&mut self, label: &str, target: String) {
        self.stored.insert(label.to_string(), target);
    }

    /// `argv` is called only for `main` without a stored target. The target stays until the
    /// window's first open is over (`remove`), so that it counts as open meanwhile.
    pub fn target_for(
        &mut self,
        label: &str,
        argv: impl FnOnce() -> Option<String>,
    ) -> Option<String> {
        self.asked.insert(label.to_string());
        match self.stored.get(label) {
            Some(target) => Some(target.clone()),
            None if label == "main" => argv(),
            None => None,
        }
    }

    /// The window's first open is over, or it was never built.
    pub fn remove(&mut self, label: &str) {
        self.stored.remove(label);
    }

    /// The stored targets, by label.
    pub fn stored(&self) -> impl Iterator<Item = (&str, &str)> {
        self.stored
            .iter()
            .map(|(label, target)| (label.as_str(), target.as_str()))
    }

    /// Whether the page of `label` has already asked: after that, a stored target is not read.
    pub fn asked(&self, label: &str) -> bool {
        self.asked.contains(label)
    }
}

/// The main windows a start reopened, until each has shown itself: then the one focused last in
/// the previous session comes to the front, whatever order their pages finished in. A window
/// shows itself (`show_window`) when its page has started, which takes the focus.
#[derive(Debug, Default)]
pub struct ReopenFocus {
    shown: HashSet<String>,
    // `None` until every window to wait for was built.
    waiting: Option<HashSet<String>>,
    focus: Option<String>,
}

impl ReopenFocus {
    /// `label` showed itself. The window to focus, once, when it was the last one waited for.
    pub fn shown(&mut self, label: &str) -> Option<String> {
        self.shown.insert(label.to_string());
        self.due()
    }

    /// Every reopened window was built: these are the ones to wait for, and `focus` is the one to
    /// focus. It may be due at once, if all of them already showed themselves.
    pub fn planned(&mut self, waiting: HashSet<String>, focus: Option<String>) -> Option<String> {
        self.waiting = Some(waiting);
        self.focus = focus;
        self.due()
    }

    fn due(&mut self) -> Option<String> {
        let waiting = self.waiting.as_ref()?;
        if waiting.is_subset(&self.shown) {
            self.focus.take()
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(label: &str) -> Option<String> {
        Some(label.to_string())
    }

    #[test]
    fn owners_of_the_first_project_window() {
        assert_eq!(owner_of("main"), s("main"));
        assert_eq!(owner_of("diff"), s("main"));
        assert_eq!(owner_of("selection-history"), s("main"));
    }

    #[test]
    fn owners_of_later_project_windows() {
        assert_eq!(owner_of("merge-3"), s("main-3"));
        assert_eq!(owner_of("viewer-12"), s("main-12"));
        assert_eq!(owner_of("selection-history-2"), s("main-2"));
    }

    #[test]
    fn malformed_labels_have_no_owner() {
        for label in [
            "main-0",
            "main-1",
            "main-02",
            "diff-",
            "other",
            "selection-history-x",
            "main-2x",
            "",
        ] {
            assert_eq!(owner_of(label), None, "{label}");
            assert_eq!(kind_of(label), None, "{label}");
        }
    }

    #[test]
    fn kinds() {
        assert_eq!(kind_of("main-2"), Some(WindowKind::Main));
        assert_eq!(
            kind_of("selection-history"),
            Some(WindowKind::SelectionHistory)
        );
        assert_eq!(kind_of("viewer-5"), Some(WindowKind::Viewer));
    }

    #[test]
    fn labels_for_an_owner() {
        assert_eq!(label_for(WindowKind::Diff, "main-2"), "diff-2");
        assert_eq!(label_for(WindowKind::Main, "main"), "main");
        assert_eq!(
            label_for(WindowKind::SelectionHistory, "main"),
            "selection-history"
        );
    }

    #[test]
    fn group_of_an_owner() {
        assert_eq!(
            owner_group("main-2"),
            [
                "main-2",
                "diff-2",
                "merge-2",
                "viewer-2",
                "selection-history-2"
            ]
        );
        assert_eq!(
            owner_group("main"),
            ["main", "diff", "merge", "viewer", "selection-history"]
        );
    }

    #[test]
    fn a_label_never_maps_to_an_owner_with_another_suffix() {
        for label in [
            "main",
            "diff",
            "merge-7",
            "viewer-12",
            "selection-history-3",
        ] {
            let owner = owner_of(label).unwrap();
            let number = |l: &str| l.rsplit_once('-').and_then(|(_, n)| n.parse::<u32>().ok());
            assert_eq!(number(label), number(&owner), "{label}");
            assert!(owner_group(&owner).contains(&label.to_string()));
        }
    }

    #[test]
    fn next_main_label_never_reuses_a_number() {
        let counter = AtomicU32::new(0);
        let mut taken = HashSet::new();
        for expected in ["main", "main-2", "main-3"] {
            let label = next_main_label(&taken, &counter);
            assert_eq!(label, expected);
            taken.insert(label);
        }
        taken.remove("main-2");
        assert_eq!(next_main_label(&taken, &counter), "main-4");
    }

    // The first `main` comes from the app's configuration, not from here: the next window is
    // still `main-2`, and the one after it `main-3`, even if `main-2` has closed.
    #[test]
    fn next_main_label_after_a_main_it_did_not_hand_out() {
        let counter = AtomicU32::new(0);
        let taken = HashSet::from(["main".to_string()]);
        assert_eq!(next_main_label(&taken, &counter), "main-2");
        assert_eq!(next_main_label(&taken, &counter), "main-3");
    }

    #[test]
    fn concurrent_calls_never_get_the_same_label() {
        let counter = AtomicU32::new(0);
        let taken = HashSet::from(["main".to_string()]);
        let labels: Vec<String> = std::thread::scope(|scope| {
            let calls: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| next_main_label(&taken, &counter)))
                .collect();
            calls.into_iter().map(|call| call.join().unwrap()).collect()
        });
        let unique: HashSet<&String> = labels.iter().collect();
        assert_eq!(unique.len(), labels.len(), "{labels:?}");
        assert!(!labels.contains(&"main".to_string()));
    }

    #[test]
    fn a_new_window_gets_its_own_initial_target() {
        let mut targets = InitialTargets::default();
        targets.store("main-2", "/work/b".to_string());
        targets.store("main-3", "/work/c".to_string());
        let argv = || Some("/work/argv".to_string());

        assert_eq!(
            targets.target_for("main", argv).as_deref(),
            Some("/work/argv")
        );
        assert_eq!(
            targets.target_for("main-2", argv).as_deref(),
            Some("/work/b")
        );
        assert_eq!(
            targets.target_for("main-3", argv).as_deref(),
            Some("/work/c")
        );
        // Only the first `main` reads the command line.
        assert_eq!(targets.target_for("main-4", argv), None);
        assert_eq!(targets.target_for("main-4", || None), None);
        assert!(targets.asked("main-2"));
        assert!(!targets.asked("main-5"));
    }

    #[test]
    fn a_stored_target_wins_over_the_command_line() {
        let mut targets = InitialTargets::default();
        assert!(!targets.asked("main"));
        targets.store("main", "/work/opened".to_string());

        let target = targets.target_for("main", || Some("/work/argv".to_string()));

        assert_eq!(target.as_deref(), Some("/work/opened"));
        assert!(targets.asked("main"));
    }

    #[test]
    fn the_last_focused_comes_to_the_front_once_every_reopened_window_showed() {
        let waiting = |labels: &[&str]| labels.iter().map(|label| label.to_string()).collect();
        let mut focus = ReopenFocus::default();
        // `main` shows before the other windows are even built.
        assert_eq!(focus.shown("main"), None);
        assert_eq!(
            focus.planned(waiting(&["main", "main-2", "main-3"]), s("main-2")),
            None
        );
        assert_eq!(focus.shown("main-2"), None);
        assert_eq!(focus.shown("main-3"), s("main-2"));
        // Only once: a window shown later (Dock, a prompt) does not move the focus again.
        assert_eq!(focus.shown("main-2"), None);

        // All of them showed before the plan was done: it is due at once.
        let mut early = ReopenFocus::default();
        early.shown("main");
        early.shown("main-2");
        assert_eq!(
            early.planned(waiting(&["main", "main-2"]), s("main")),
            s("main")
        );

        // Without a last focused one, nothing is focused.
        let mut none = ReopenFocus::default();
        assert_eq!(none.planned(waiting(&["main"]), None), None);
        assert_eq!(none.shown("main"), None);
    }
}
