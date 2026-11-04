// The `git-rebase-todo` file and the model it is edited with. Ported from `GitRebaseEntry`,
// `GitInteractiveRebaseFile`, `GitRebaseTodoModel` and `GitRebaseTodoModelConverter`.
use std::fmt;

use serde::{Deserialize, Serialize};

// Git's help comes marked with it (`editor::REBASE_CONFIG`).
const COMMENT_CHAR: char = '\u{1}';
const NOOP: &str = "noop";

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "String", from = "String")]
pub enum Action {
    Pick,
    Edit,
    Drop,
    Squash,
    Reword,
    Fixup,
    Other(String),
}

impl Action {
    pub fn command(&self) -> &str {
        match self {
            Self::Pick => "pick",
            Self::Edit => "edit",
            Self::Drop => "drop",
            Self::Squash => "squash",
            Self::Reword => "reword",
            Self::Fixup => "fixup",
            Self::Other(command) => command,
        }
    }
}

// `Action.fromString`: only whole words; `p` or `f` are `Other`, as in IntelliJ.
impl From<String> for Action {
    fn from(command: String) -> Self {
        match command.as_str() {
            "pick" => Self::Pick,
            "edit" => Self::Edit,
            "drop" => Self::Drop,
            "squash" => Self::Squash,
            "reword" => Self::Reword,
            "fixup" => Self::Fixup,
            _ => Self::Other(command),
        }
    }
}

impl From<Action> for String {
    fn from(action: Action) -> Self {
        action.command().to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub action: Action,
    // As git writes it: abbreviated.
    pub hash: String,
    pub subject: String,
}

impl Entry {
    // The full commit is this one if it starts with the line's abbreviated hash.
    pub fn is(&self, full: &str) -> bool {
        !self.hash.is_empty() && full.starts_with(&self.hash)
    }
}

// `GitRebaseEntry.toString`, without the trailing space if there is no subject (`update-ref`, `break`).
impl fmt::Display for Entry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut line = format!("{} {}", self.action.command(), self.hash);
        if !self.subject.is_empty() {
            line.push(' ');
            line.push_str(&self.subject);
        }
        f.write_str(line.trim_end())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TodoError {
    // Only `noop`: there is nothing to rebase.
    Noop,
    // An action the model does not know: it is edited as text, like `handleUnstructuredEditor`.
    Unstructured,
    Invalid(String),
}

// `StringScanner.spaceToken`: up to the first space, which is skipped.
fn token(line: &str) -> (&str, &str) {
    line.split_once(' ').unwrap_or((line, ""))
}

// `GitInteractiveRebaseFile.load`.
pub fn parse(text: &str) -> Result<Vec<Entry>, TodoError> {
    let mut entries = Vec::new();
    let mut noop = false;
    for line in text.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() || line.starts_with(COMMENT_CHAR) {
            continue;
        }
        if line.starts_with(NOOP) {
            noop = true;
            continue;
        }
        let (action, rest) = token(line);
        let (hash, subject) = token(rest);
        entries.push(Entry {
            action: Action::from(action.to_string()),
            hash: hash.to_string(),
            subject: subject.to_string(),
        });
    }
    if noop && entries.is_empty() {
        return Err(TodoError::Noop);
    }
    Ok(entries)
}

// `GitInteractiveRebaseFile.save`: one line per entry.
pub fn write(entries: &[Entry]) -> String {
    entries.iter().map(|entry| format!("{entry}\n")).collect()
}

// `GitRebaseTodoModel.Type`. `Pick`, `Edit` and `Reword` keep the commit (`KeepCommit`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Pick,
    Edit,
    Reword(String),
    Drop,
}

impl Type {
    fn keeps_commit(&self) -> bool {
        !matches!(self, Self::Drop)
    }

    fn action(&self) -> Action {
        match self {
            Self::Pick => Action::Pick,
            Self::Edit => Action::Edit,
            Self::Reword(_) => Action::Reword,
            Self::Drop => Action::Drop,
        }
    }
}

// `Element.Simple`, `UniteRoot` and `UniteChild`. Each element carries a fixed `id` (its line's),
// which stands in for the object identity of Kotlin: the root keeps those of its children, and the
// child, that of its root.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    Simple(Type),
    Root { kind: Type, children: Vec<usize> },
    Child { root: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Element {
    id: usize,
    entry: Entry,
    kind: Kind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    rows: Vec<Element>,
}

fn invalid(text: &str) -> TodoError {
    TodoError::Invalid(text.to_string())
}

impl Model {
    // `convertToModel`: the `drop`s go to the end, and `fixup` and `squash` unite with the previous one.
    pub fn from_entries(entries: Vec<Entry>) -> Result<Self, TodoError> {
        let mut rows: Vec<Element> = Vec::new();
        let mut dropped = Vec::new();
        for (id, entry) in entries.into_iter().enumerate() {
            let kind = match entry.action {
                Action::Pick | Action::Reword => Kind::Simple(Type::Pick),
                Action::Edit => Kind::Simple(Type::Edit),
                Action::Drop => {
                    dropped.push(Element {
                        id,
                        entry,
                        kind: Kind::Simple(Type::Drop),
                    });
                    continue;
                }
                Action::Fixup | Action::Squash => {
                    let last = rows
                        .len()
                        .checked_sub(1)
                        .ok_or_else(|| invalid("Couldn't unite with non-existed commit"))?;
                    let root = match &rows[last].kind {
                        Kind::Child { root } => *root,
                        Kind::Root { .. } => rows[last].id,
                        Kind::Simple(kind) if kind.keeps_commit() => {
                            rows[last].kind = Kind::Root {
                                kind: kind.clone(),
                                children: Vec::new(),
                            };
                            rows[last].id
                        }
                        Kind::Simple(_) => return Err(invalid("Couldn't unite with a drop")),
                    };
                    let position = rows.iter().position(|row| row.id == root).unwrap_or(last);
                    if let Kind::Root { children, .. } = &mut rows[position].kind {
                        children.push(id);
                    }
                    Kind::Child { root }
                }
                Action::Other(_) => return Err(TodoError::Unstructured),
            };
            rows.push(Element { id, entry, kind });
        }
        rows.extend(dropped);
        Ok(Self { rows })
    }

    // `convertToEntries`: the children of a group are written `fixup`.
    pub fn to_entries(&self) -> Vec<Entry> {
        self.rows
            .iter()
            .map(|row| Entry {
                action: match &row.kind {
                    Kind::Simple(kind) | Kind::Root { kind, .. } => kind.action(),
                    Kind::Child { .. } => Action::Fixup,
                },
                hash: row.entry.hash.clone(),
                subject: row.entry.subject.clone(),
            })
            .collect()
    }

    // Those left as `reword`, in order, with their new message.
    pub fn reworded(&self) -> Vec<(&Entry, &str)> {
        self.rows
            .iter()
            .filter_map(|row| match &row.kind {
                Kind::Simple(Type::Reword(message))
                | Kind::Root {
                    kind: Type::Reword(message),
                    ..
                } => Some((&row.entry, message.as_str())),
                _ => None,
            })
            .collect()
    }

    pub fn index_of(&self, hash: &str) -> Result<usize, TodoError> {
        self.rows
            .iter()
            .position(|row| row.entry.is(hash))
            .ok_or_else(|| {
                TodoError::Invalid(format!("The commit \"{hash}\" is not in the rebase list"))
            })
    }

    fn indices(&self, hashes: &[&str]) -> Result<Vec<usize>, TodoError> {
        hashes.iter().map(|hash| self.index_of(hash)).collect()
    }

    pub fn drop(&mut self, hashes: &[&str]) -> Result<(), TodoError> {
        let indices = self.indices(hashes)?;
        self.modify(|rows| rows.drop(&indices))
    }

    pub fn reword(&mut self, hash: &str, message: &str) -> Result<(), TodoError> {
        let index = self.index_of(hash)?;
        if matches!(self.rows[index].kind, Kind::Child { .. }) {
            return Err(invalid("A commit united with another cannot be edited"));
        }
        let kind = Type::Reword(message.to_string());
        self.modify(|rows| rows.keep_commit(&[index], kind))
    }

    // `unite`: the children go behind the root, in the given order.
    pub fn unite(&mut self, root: &str, children: &[&str]) -> Result<(), TodoError> {
        let mut indices = vec![self.index_of(root)?];
        indices.extend(self.indices(children)?);
        if indices.len() < 2 {
            return Err(invalid("At least two commits are needed"));
        }
        self.modify(|rows| rows.unite(&indices).map(drop))
    }

    // `exchangeIndices`: the commit (with its group, if it is a root) moves to `index`.
    pub fn reorder(&mut self, hash: &str, index: usize) -> Result<(), TodoError> {
        let old = self.index_of(hash)?;
        if index >= self.rows.len() {
            return Err(invalid("Position outside the list"));
        }
        self.modify(|rows| rows.exchange(old, index))
    }

    // `ElementList.modifyList`: the list only changes if it stays well formed.
    fn modify(
        &mut self,
        change: impl FnOnce(&mut Rows) -> Result<(), TodoError>,
    ) -> Result<(), TodoError> {
        let mut rows = Rows(self.rows.clone());
        change(&mut rows)?;
        rows.sort_children();
        rows.validate()?;
        self.rows = rows.0;
        Ok(())
    }
}

// `MutableElementList` and the operations of `GitRebaseTodoModel` on it, by position.
struct Rows(Vec<Element>);

impl Rows {
    fn at(&self, id: usize) -> usize {
        self.0
            .iter()
            .position(|row| row.id == id)
            .unwrap_or(usize::MAX)
    }

    fn children(&self, root: usize) -> Vec<usize> {
        match self.0.get(self.at(root)).map(|row| &row.kind) {
            Some(Kind::Root { children, .. }) => children.clone(),
            _ => Vec::new(),
        }
    }

    fn root_type(&self, root: usize) -> Type {
        match self.0.get(self.at(root)).map(|row| &row.kind) {
            Some(Kind::Root { kind, .. }) => kind.clone(),
            _ => Type::Pick,
        }
    }

    fn group(&self, root: usize) -> Vec<usize> {
        let mut group = vec![root];
        let mut children = self.children(root);
        children.sort_by_key(|child| self.at(*child));
        group.extend(children);
        group
    }

    fn last_child_index(&self, root: usize) -> usize {
        self.children(root)
            .iter()
            .map(|child| self.at(*child))
            .max()
            .unwrap_or_else(|| self.at(root))
    }

    fn set(&mut self, index: usize, kind: Kind) {
        self.0[index].kind = kind;
    }

    fn remove_child(&mut self, root: usize, child: usize) {
        let index = self.at(root);
        if let Some(Kind::Root { children, .. }) = self.0.get_mut(index).map(|row| &mut row.kind) {
            children.retain(|id| *id != child);
        }
    }

    fn add_child(&mut self, root: usize, child: usize) -> Result<(), TodoError> {
        let (first, next) = (self.at(root) + 1, self.last_child_index(root) + 1);
        if !(first..=next).contains(&self.at(child)) {
            return Err(invalid("The child is not next to its group"));
        }
        let index = self.at(root);
        if let Some(Kind::Root { children, .. }) = self.0.get_mut(index).map(|row| &mut row.kind) {
            children.push(child);
        }
        Ok(())
    }

    // `convertUniteGroupToSimple`.
    fn group_to_simple(&mut self, root: usize, kind: &Type) {
        for id in self.group(root) {
            let index = self.at(id);
            self.set(index, Kind::Simple(kind.clone()));
        }
    }

    // `changeUniteChild`: if the root is left without children, it becomes simple again with its type.
    fn change_child(&mut self, index: usize, kind: Kind) {
        if let Kind::Child { root } = self.0[index].kind {
            self.remove_child(root, self.0[index].id);
            if self.children(root).is_empty() {
                let root_type = self.root_type(root);
                let at = self.at(root);
                self.set(at, Kind::Simple(root_type));
            }
        }
        self.set(index, kind);
    }

    // `removeAndMoveUniteChild`: the child leaves the group and stays right behind it.
    fn remove_and_move_child(&mut self, index: usize, kind: Type) -> Result<usize, TodoError> {
        let Kind::Child { root } = self.0[index].kind else {
            return Ok(index);
        };
        let id = self.0[index].id;
        let target = self.last_child_index(root);
        self.change_child(index, Kind::Simple(kind));
        self.move_elements(&[id], target)?;
        Ok(self.at(id))
    }

    // `keepCommitAction`.
    fn keep_commit(&mut self, indices: &[usize], kind: Type) -> Result<(), TodoError> {
        let mut sorted = indices.to_vec();
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        for index in sorted {
            match &mut self.0[index].kind {
                Kind::Simple(current) | Kind::Root { kind: current, .. } => *current = kind.clone(),
                Kind::Child { .. } => {
                    self.remove_and_move_child(index, kind.clone())?;
                }
            }
        }
        Ok(())
    }

    fn drop(&mut self, indices: &[usize]) -> Result<(), TodoError> {
        let elements: Vec<(usize, Kind)> = indices
            .iter()
            .map(|index| (self.0[*index].id, self.0[*index].kind.clone()))
            .collect();
        for (id, kind) in &elements {
            if matches!(kind, Kind::Simple(_)) {
                let index = self.at(*id);
                self.set(index, Kind::Simple(Type::Drop));
            }
        }
        for (id, kind) in &elements {
            if matches!(kind, Kind::Root { .. }) {
                self.group_to_simple(*id, &Type::Drop);
            }
        }
        // The children that remain leave the group, from the bottom up so as not to move the indices.
        let mut sorted = indices.to_vec();
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        let children: Vec<usize> = sorted
            .iter()
            .filter(|index| matches!(self.0[**index].kind, Kind::Child { .. }))
            .map(|index| self.0[*index].id)
            .collect();
        for child in children {
            self.remove_and_move_child(self.at(child), Type::Drop)?;
        }
        Ok(())
    }

    // `convertToRoot`.
    fn convert_to_root(&mut self, index: usize) -> usize {
        match &self.0[index].kind {
            Kind::Root { .. } => self.0[index].id,
            Kind::Child { root } => *root,
            Kind::Simple(current) => {
                let kind = if current.keeps_commit() {
                    current.clone()
                } else {
                    Type::Pick
                };
                self.set(
                    index,
                    Kind::Root {
                        kind,
                        children: Vec::new(),
                    },
                );
                self.0[index].id
            }
        }
    }

    // `addToUniteGroup`.
    fn add_to_group(&mut self, index: usize, root: usize) -> Result<(), TodoError> {
        let id = self.0[index].id;
        match self.0[index].kind.clone() {
            Kind::Child { root: current } if current == root => return Ok(()),
            Kind::Child { .. } => self.change_child(index, Kind::Child { root }),
            Kind::Root { .. } => {
                self.group_to_simple(id, &Type::Pick);
                self.set(index, Kind::Child { root });
            }
            Kind::Simple(_) => self.set(index, Kind::Child { root }),
        }
        self.add_child(root, id)
    }

    fn unite(&mut self, indices: &[usize]) -> Result<usize, TodoError> {
        let root = self.convert_to_root(indices[0]);
        let mut moved: Vec<usize> = Vec::new();
        for index in &indices[1..] {
            let element = &self.0[*index];
            let group = match &element.kind {
                Kind::Child { root: current } if *current == root => continue,
                Kind::Root { .. } => self.group(element.id),
                _ => vec![element.id],
            };
            for id in group {
                if !moved.contains(&id) {
                    moved.push(id);
                }
            }
        }
        let target = self.last_child_index(root) + 1;
        self.move_elements(&moved, target)?;
        for id in moved {
            self.add_to_group(self.at(id), root)?;
        }
        Ok(root)
    }

    fn exchange(&mut self, old: usize, new: usize) -> Result<(), TodoError> {
        let element = self.0[old].clone();
        let moving = match element.kind {
            Kind::Root { .. } => self.group(element.id),
            Kind::Simple(_) => vec![element.id],
            Kind::Child { root } => {
                let index = self.remove_and_move_child(old, Type::Pick)?;
                if index == new {
                    self.add_to_group(index, root)?;
                    Vec::new()
                } else {
                    vec![element.id]
                }
            }
        };
        self.move_elements(&moving, new)?;
        // `addToUniteGroupIfNeeded`: what falls inside a group is united to it.
        let Some(last) = moving.last() else {
            return Ok(());
        };
        if let Some(Kind::Child { root }) =
            self.0.get(self.at(*last) + 1).map(|row| row.kind.clone())
        {
            for id in &moving {
                self.add_to_group(self.at(*id), root)?;
            }
        }
        Ok(())
    }

    // `moveElements`: the group comes to start at `position` and what is in between shifts.
    fn move_elements(&mut self, group: &[usize], position: usize) -> Result<(), TodoError> {
        let (Some(first), Some(last)) = (group.first(), group.last()) else {
            return Ok(());
        };
        let size = self.0.len();
        if group.len() > size {
            return Err(invalid("Group larger than the list"));
        }
        let (first, last) = (self.at(*first), self.at(*last));
        let min = first.min(position);
        let max = last.max((position + group.len()).min(size - 1));
        let new_first = position.min(size - group.len());
        let indices: Vec<usize> = group.iter().map(|id| self.at(*id)).collect();
        let saved: Vec<Element> = (min..=max)
            .filter(|index| !indices.contains(index))
            .map(|index| self.0[index].clone())
            .collect();
        let before = new_first
            .checked_sub(min)
            .filter(|count| *count <= saved.len());
        let Some(before) = before else {
            return Err(invalid("Move outside the list"));
        };
        let moved: Vec<Element> = indices.iter().map(|index| self.0[*index].clone()).collect();
        let interval: Vec<Element> = saved[..before]
            .iter()
            .cloned()
            .chain(moved)
            .chain(saved[before..].iter().cloned())
            .collect();
        if interval.len() != max - min + 1 {
            return Err(invalid("The group does not fit where it is moved"));
        }
        for (offset, element) in interval.into_iter().enumerate() {
            self.0[min + offset] = element;
        }
        Ok(())
    }

    // `childrenIndicesChanged`.
    fn sort_children(&mut self) {
        let positions: Vec<(usize, usize)> = self
            .0
            .iter()
            .enumerate()
            .map(|(index, row)| (row.id, index))
            .collect();
        let at = |id: &usize| {
            positions
                .iter()
                .find(|(row, _)| row == id)
                .map_or(usize::MAX, |(_, index)| *index)
        };
        for row in &mut self.0 {
            if let Kind::Root { children, .. } = &mut row.kind {
                children.sort_by_key(at);
            }
        }
    }

    // `validateElements`: each group, with its children right behind the root.
    fn validate(&self) -> Result<(), TodoError> {
        for (index, row) in self.0.iter().enumerate() {
            match &row.kind {
                Kind::Root { children, .. } => {
                    if children.is_empty() {
                        return Err(invalid("A group without children"));
                    }
                    for (offset, child) in children.iter().enumerate() {
                        let next = self.0.get(index + offset + 1);
                        let fits = next.is_some_and(|next| {
                            next.id == *child && next.kind == Kind::Child { root: row.id }
                        });
                        if !fits {
                            return Err(invalid("A group with its children apart"));
                        }
                    }
                }
                Kind::Child { root } => {
                    if !self.children(*root).contains(&row.id) {
                        return Err(invalid("A child without a group"));
                    }
                }
                Kind::Simple(_) => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(action: Action, hash: &str, subject: &str) -> Entry {
        Entry {
            action,
            hash: hash.to_string(),
            subject: subject.to_string(),
        }
    }

    fn picks(hashes: &[&str]) -> Vec<Entry> {
        hashes
            .iter()
            .map(|hash| entry(Action::Pick, hash, &format!("# {hash}")))
            .collect()
    }

    fn lines(model: &Model) -> Vec<String> {
        model.to_entries().iter().map(Entry::to_string).collect()
    }

    #[test]
    fn parse_and_write_round_trip() {
        let text = "pick a1b2c3d # one\n\
                    reword e4f5a6b # two with  spaces\n\
                    \n\
                    \u{1} Rebase a1b2..e4f5 onto 0000 (2 commands)\n\
                    \u{1}\n\
                    update-ref refs/heads/other\n\
                    fixup -C 1234567 three\n";
        let entries = parse(text).unwrap();

        assert_eq!(
            entries,
            vec![
                entry(Action::Pick, "a1b2c3d", "# one"),
                entry(Action::Reword, "e4f5a6b", "# two with  spaces"),
                entry(
                    Action::Other("update-ref".to_string()),
                    "refs/heads/other",
                    ""
                ),
                entry(Action::Fixup, "-C", "1234567 three"),
            ]
        );
        assert_eq!(
            write(&entries),
            "pick a1b2c3d # one\nreword e4f5a6b # two with  spaces\nupdate-ref refs/heads/other\nfixup -C 1234567 three\n"
        );
        assert_eq!(parse(&write(&entries)).unwrap(), entries);
        // A line that starts with `#` is not help: with `core.commentChar` the help is `\u{1}`.
        assert_eq!(
            parse("# not help\n").unwrap(),
            vec![entry(Action::Other("#".to_string()), "not", "help")]
        );
        assert_eq!(parse("noop\n\u{1} help\n"), Err(TodoError::Noop));
        assert_eq!(
            parse("noop\npick a1 one\n").unwrap(),
            vec![entry(Action::Pick, "a1", "one")]
        );
        assert_eq!(parse("").unwrap(), Vec::new());
        // The actions are served to the interface as their git command.
        assert_eq!(
            serde_json::to_string(&entry(Action::Other("exec".into()), "make", "")).unwrap(),
            r#"{"action":"exec","hash":"make","subject":""}"#
        );
        assert_eq!(
            serde_json::from_str::<Action>(r#""fixup""#).unwrap(),
            Action::Fixup
        );
    }

    #[test]
    fn model_moves_drops_to_the_end_and_unites_fixup_and_squash() {
        let mut entries = picks(&["a", "b", "c", "d"]);
        entries[1].action = Action::Drop;
        entries[3].action = Action::Squash;
        let model = Model::from_entries(entries).unwrap();

        assert_eq!(
            lines(&model),
            ["pick a # a", "pick c # c", "fixup d # d", "drop b # b"]
        );
        let mut other = picks(&["a"]);
        other.push(entry(Action::Other("exec".into()), "make", ""));
        assert_eq!(Model::from_entries(other), Err(TodoError::Unstructured));
        let mut alone = picks(&["a"]);
        alone[0].action = Action::Fixup;
        assert!(matches!(
            Model::from_entries(alone),
            Err(TodoError::Invalid(_))
        ));
    }

    #[test]
    fn non_contiguous_unite_puts_the_children_after_the_root() {
        let mut model = Model::from_entries(picks(&["a", "b", "c", "d"])).unwrap();

        model.unite("a", &["c"]).unwrap();
        model.reword("a", "united").unwrap();

        assert_eq!(
            lines(&model),
            ["reword a # a", "fixup c # c", "pick b # b", "pick d # d"]
        );
        assert_eq!(model.reworded(), vec![(&model.rows[0].entry, "united")]);
        // Uniting a group with another commit extends it.
        model.unite("a", &["d"]).unwrap();
        assert_eq!(
            lines(&model),
            ["reword a # a", "fixup c # c", "fixup d # d", "pick b # b"]
        );
    }

    #[test]
    fn drop_of_a_child_takes_it_out_of_the_group_and_of_a_root_releases_the_group() {
        let mut model = Model::from_entries(picks(&["a", "b", "c", "d"])).unwrap();
        model.unite("a", &["b", "c"]).unwrap();

        model.drop(&["b"]).unwrap();
        assert_eq!(
            lines(&model),
            ["pick a # a", "fixup c # c", "drop b # b", "pick d # d"]
        );

        model.drop(&["a"]).unwrap();
        assert_eq!(
            lines(&model),
            ["drop a # a", "drop c # c", "drop b # b", "pick d # d"]
        );
        assert!(matches!(model.drop(&["zz"]), Err(TodoError::Invalid(_))));
    }

    #[test]
    fn reorder_moves_the_whole_group_and_what_falls_inside_is_united() {
        let mut model = Model::from_entries(picks(&["a", "b", "c", "d"])).unwrap();
        model.unite("b", &["c"]).unwrap();

        model.reorder("b", 0).unwrap();
        assert_eq!(
            lines(&model),
            ["pick b # b", "fixup c # c", "pick a # a", "pick d # d"]
        );

        model.reorder("d", 1).unwrap();
        assert_eq!(
            lines(&model),
            ["pick b # b", "fixup d # d", "fixup c # c", "pick a # a"]
        );
        // A child that moves out of its group becomes `pick` again.
        model.reorder("c", 3).unwrap();
        assert_eq!(
            lines(&model),
            ["pick b # b", "fixup d # d", "pick a # a", "pick c # c"]
        );
    }
}
