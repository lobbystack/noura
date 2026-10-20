//! The files service: the file tree's view state and opening files.

use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use local_core::WorkspaceEntryKind;
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

use crate::{AppError, Workspace, error::Access};

/// Typed characters within this window extend the type-ahead search.
pub const TYPE_AHEAD_TIMEOUT: Duration = Duration::from_millis(1000);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Folder,
    File,
}

/// One visible row of the file tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    /// The name the tree shows: Markdown files appear without `.md`.
    pub name: String,
    pub relative_path: String,
    pub kind: EntryKind,
    /// How deep the row sits: 0 for top-level rows.
    pub depth: usize,
    /// Whether a folder is open. `None` for files.
    pub expanded: Option<bool>,
    /// The file lives only in cloud storage, such as iCloud, and isn't on
    /// this device yet.
    pub not_downloaded: bool,
}

/// A key the file tree understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeKey {
    Up,
    Down,
    Home,
    End,
    /// Open a closed folder, or move into an open one.
    Right,
    /// Close an open folder, or move to the parent folder.
    Left,
    /// Enter or Space: open a file or toggle a folder.
    Activate,
}

/// What the tree asks its owner to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeAction {
    OpenFile {
        relative_path: String,
    },
    /// Move the item at `relative_path` into `folder` (`""` is the top of
    /// the workspace).
    Move {
        relative_path: String,
        folder: String,
    },
}

/// Resting on a closed folder this long while dragging opens it.
pub const SPRING_OPEN_DELAY: Duration = Duration::from_millis(1000);

/// Where the pointer is while a row is dragged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropSpot {
    /// Over the visible row at this index.
    Row(usize),
    /// Over the tree but below its rows: the top of the workspace.
    Blank,
    /// Outside the tree.
    Away,
}

/// A row being dragged.
struct Drag {
    node: usize,
    /// The closed folder under the pointer and when the pointer got there.
    resting: Option<(usize, Instant)>,
    /// The folder a drop would move the row into.
    target: Option<String>,
    /// Folders this drag opened, which close again unless they hold the
    /// drop.
    opened: Vec<usize>,
}

/// A file shown in the document area.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedFile {
    /// The file's title: its name without `.md` for Markdown files.
    pub name: String,
    pub relative_path: String,
}

/// One file or folder from the workspace listing.
pub(crate) struct TreeEntry {
    pub(crate) relative_path: String,
    pub(crate) kind: EntryKind,
    pub(crate) not_downloaded: bool,
}

struct Node {
    file_name: String,
    /// The file name folded for sorting: lowercase, without accents.
    sort_key: String,
    shown: String,
    relative_path: String,
    kind: EntryKind,
    not_downloaded: bool,
    children: Vec<usize>,
}

/// The file tree of one workspace: its folders and files, which folders are
/// open, the visible rows and the keyboard focus.
pub struct FileTree {
    nodes: Vec<Node>,
    top: Vec<usize>,
    expanded: HashSet<usize>,
    rows: Vec<TreeRow>,
    /// The node behind each visible row.
    row_nodes: Vec<usize>,
    parents: Vec<Option<usize>>,
    /// The node at each workspace path.
    by_path: HashMap<String, usize>,
    focused: Option<usize>,
    typed: String,
    typed_at: Option<Instant>,
    drag: Option<Drag>,
}

impl FileTree {
    pub(crate) fn new(entries: impl IntoIterator<Item = TreeEntry>) -> Self {
        let mut tree = Self {
            nodes: Vec::new(),
            top: Vec::new(),
            expanded: HashSet::new(),
            rows: Vec::new(),
            row_nodes: Vec::new(),
            parents: Vec::new(),
            by_path: HashMap::new(),
            focused: None,
            typed: String::new(),
            typed_at: None,
            drag: None,
        };
        let mut folders: HashMap<String, usize> = HashMap::new();
        for entry in entries {
            if entry.kind == EntryKind::Folder {
                tree.folder(&entry.relative_path, &mut folders);
                continue;
            }
            let parent =
                parent_of(&entry.relative_path).map(|path| tree.folder(path, &mut folders));
            let index = tree.add(entry.relative_path, EntryKind::File, parent);
            tree.nodes[index].not_downloaded = entry.not_downloaded;
        }
        let order: Vec<usize> = (0..tree.nodes.len()).collect();
        for index in order {
            let mut children = std::mem::take(&mut tree.nodes[index].children);
            children.sort_by(|left, right| tree.compare(*left, *right));
            tree.nodes[index].children = children;
        }
        let mut top = std::mem::take(&mut tree.top);
        top.sort_by(|left, right| tree.compare(*left, *right));
        tree.top = top;
        tree.refresh_rows();
        tree
    }

    fn folder(&mut self, path: &str, folders: &mut HashMap<String, usize>) -> usize {
        if let Some(index) = folders.get(path) {
            return *index;
        }
        let parent = parent_of(path).map(|parent| self.folder(parent, folders));
        let index = self.add(path.to_owned(), EntryKind::Folder, parent);
        folders.insert(path.to_owned(), index);
        index
    }

    fn add(&mut self, relative_path: String, kind: EntryKind, parent: Option<usize>) -> usize {
        let file_name = relative_path
            .rsplit_once('/')
            .map_or(relative_path.as_str(), |(_, name)| name)
            .to_owned();
        let index = self.nodes.len();
        self.nodes.push(Node {
            shown: shown_name(&file_name, kind),
            sort_key: sort_key(&file_name),
            file_name,
            relative_path,
            kind,
            not_downloaded: false,
            children: Vec::new(),
        });
        self.parents.push(parent);
        self.by_path
            .insert(self.nodes[index].relative_path.clone(), index);
        match parent {
            Some(parent) => self.nodes[parent].children.push(index),
            None => self.top.push(index),
        }
        index
    }

    fn compare(&self, left: usize, right: usize) -> Ordering {
        let (left, right) = (&self.nodes[left], &self.nodes[right]);
        let rank = |node: &Node| match node.kind {
            EntryKind::Folder => 0,
            EntryKind::File => 1,
        };
        rank(left)
            .cmp(&rank(right))
            .then_with(|| natural_cmp(&left.sort_key, &right.sort_key))
            .then_with(|| left.file_name.cmp(&right.file_name))
    }

    fn refresh_rows(&mut self) {
        let focused_node = self.focused.map(|row| self.row_nodes[row]);
        self.rows.clear();
        self.row_nodes.clear();
        let mut stack: Vec<(usize, usize)> = self.top.iter().rev().map(|node| (*node, 0)).collect();
        while let Some((index, depth)) = stack.pop() {
            let node = &self.nodes[index];
            let open = self.expanded.contains(&index);
            self.rows.push(TreeRow {
                name: node.shown.clone(),
                relative_path: node.relative_path.clone(),
                kind: node.kind,
                depth,
                expanded: (node.kind == EntryKind::Folder).then_some(open),
                not_downloaded: node.not_downloaded,
            });
            self.row_nodes.push(index);
            if open {
                stack.extend(node.children.iter().rev().map(|child| (*child, depth + 1)));
            }
        }
        self.focused = focused_node.and_then(|node| self.row_of(node));
    }

    fn row_of(&self, node: usize) -> Option<usize> {
        self.row_nodes
            .iter()
            .position(|candidate| *candidate == node)
    }

    /// The visible rows, top to bottom.
    pub fn rows(&self) -> &[TreeRow] {
        &self.rows
    }

    /// The row with keyboard focus.
    pub fn focused(&self) -> Option<usize> {
        self.focused
    }

    /// The visible row showing `relative_path`.
    pub fn find_row(&self, relative_path: &str) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| row.relative_path == relative_path)
    }

    pub fn set_focus(&mut self, row: usize) {
        if row < self.rows.len() {
            self.focused = Some(row);
        }
    }

    /// Open the folders above the item at `relative_path` and make its row
    /// the current row. Returns whether the tree shows the item.
    pub fn select(&mut self, relative_path: &str) -> bool {
        let Some(&node) = self.by_path.get(relative_path) else {
            return false;
        };
        let mut parent = self.parents[node];
        while let Some(folder) = parent {
            self.expanded.insert(folder);
            parent = self.parents[folder];
        }
        self.refresh_rows();
        self.focused = self.row_of(node);
        true
    }

    /// The folder new notes and folders go in: the current row when it is
    /// a folder, the current file's folder, or the top of the workspace
    /// (`""`) when no row is current.
    pub fn target_folder(&self) -> String {
        let Some(row) = self.focused.and_then(|row| self.rows.get(row)) else {
            return String::new();
        };
        match row.kind {
            EntryKind::Folder => row.relative_path.clone(),
            EntryKind::File => parent_of(&row.relative_path).unwrap_or("").to_owned(),
        }
    }

    /// Close every folder. The current row moves to the top-level row that
    /// held it.
    pub fn collapse_all(&mut self) {
        let focused = self.focused.map(|row| {
            let mut node = self.row_nodes[row];
            while let Some(parent) = self.parents[node] {
                node = parent;
            }
            node
        });
        self.expanded.clear();
        self.refresh_rows();
        self.focused = focused.and_then(|node| self.row_of(node));
    }

    /// Start dragging the row at `row`.
    pub fn start_drag(&mut self, row: usize) {
        self.cancel_drag();
        if let Some(&node) = self.row_nodes.get(row) {
            self.drag = Some(Drag {
                node,
                resting: None,
                target: None,
                opened: Vec::new(),
            });
        }
    }

    /// The workspace path of the row being dragged.
    pub fn dragging(&self) -> Option<&str> {
        self.drag
            .as_ref()
            .map(|drag| self.nodes[drag.node].relative_path.as_str())
    }

    /// The pointer is at `spot` at time `at` while a row is dragged. Over a
    /// folder the drop goes into it, over a file into the file's folder,
    /// and over the blank part of the tree to the top of the workspace.
    /// Resting on a closed folder for [`SPRING_OPEN_DELAY`] opens it.
    pub fn drag_over(&mut self, spot: DropSpot, at: Instant) {
        let Some(drag) = &self.drag else {
            return;
        };
        let dragged = drag.node;
        let over = match spot {
            DropSpot::Row(row) => self.row_nodes.get(row).copied(),
            DropSpot::Blank | DropSpot::Away => None,
        };
        let folder = match (spot, over) {
            (DropSpot::Away, _) | (DropSpot::Row(_), None) => None,
            (DropSpot::Blank, _) => Some(String::new()),
            (DropSpot::Row(_), Some(node)) => Some(match self.nodes[node].kind {
                EntryKind::Folder => self.nodes[node].relative_path.clone(),
                EntryKind::File => parent_of(&self.nodes[node].relative_path)
                    .unwrap_or("")
                    .to_owned(),
            }),
        };
        let target =
            folder.filter(|folder| can_move_into(&self.nodes[dragged].relative_path, folder));
        let closed_folder = over.filter(|node| {
            self.nodes[*node].kind == EntryKind::Folder && !self.expanded.contains(node)
        });
        let Some(drag) = &mut self.drag else {
            return;
        };
        drag.target = target;
        drag.resting = match (closed_folder, drag.resting) {
            (Some(node), Some((resting, since))) if node == resting => Some((node, since)),
            (Some(node), _) => Some((node, at)),
            (None, _) => None,
        };
        if let Some((node, since)) = drag.resting
            && at.saturating_duration_since(since) >= SPRING_OPEN_DELAY
        {
            drag.resting = None;
            drag.opened.push(node);
            self.expanded.insert(node);
            self.refresh_rows();
        }
    }

    /// The folder a drop would move the dragged row into, if any.
    pub fn drop_target(&self) -> Option<&str> {
        self.drag.as_ref().and_then(|drag| drag.target.as_deref())
    }

    /// When the closed folder under the pointer opens, if the pointer
    /// stays.
    pub fn spring_open_at(&self) -> Option<Instant> {
        self.drag
            .as_ref()
            .and_then(|drag| drag.resting)
            .map(|(_, since)| since + SPRING_OPEN_DELAY)
    }

    /// Drop the dragged row. Folders the drag opened close again unless
    /// they hold the drop. Returns the move to make, if the drop landed
    /// somewhere the row can go.
    pub fn finish_drag(&mut self) -> Option<TreeAction> {
        let drag = self.drag.take()?;
        let target = drag.target.clone();
        self.close_opened(&drag, target.as_deref());
        target.map(|folder| TreeAction::Move {
            relative_path: self.nodes[drag.node].relative_path.clone(),
            folder,
        })
    }

    /// Stop dragging without moving anything, and close the folders the
    /// drag opened.
    pub fn cancel_drag(&mut self) {
        if let Some(drag) = self.drag.take() {
            self.close_opened(&drag, None);
        }
    }

    fn close_opened(&mut self, drag: &Drag, kept: Option<&str>) {
        if drag.opened.is_empty() {
            return;
        }
        for folder in &drag.opened {
            let path = &self.nodes[*folder].relative_path;
            let holds_drop = kept.is_some_and(|kept| remap(kept, path, path).is_some());
            if !holds_drop {
                self.expanded.remove(folder);
            }
        }
        self.refresh_rows();
    }

    /// Whether the item at `relative_path` is a folder in the tree.
    pub fn is_folder(&self, relative_path: &str) -> bool {
        self.by_path
            .get(relative_path)
            .is_some_and(|node| self.nodes[*node].kind == EntryKind::Folder)
    }

    /// Take over `old`'s open folders and current row. When an item moved
    /// from `moved.0` to `moved.1`, its open folders and the current row
    /// follow it.
    pub(crate) fn carry_over(&mut self, old: &FileTree, moved: Option<(&str, &str)>) {
        let follow = |path: &str| match moved {
            Some((from, to)) => remap(path, from, to).unwrap_or_else(|| path.to_owned()),
            None => path.to_owned(),
        };
        for node in &old.expanded {
            let path = follow(&old.nodes[*node].relative_path);
            if let Some(&index) = self.by_path.get(&path) {
                self.expanded.insert(index);
            }
        }
        self.refresh_rows();
        if let Some(row) = old.focused {
            let path = follow(&old.rows[row].relative_path);
            if let Some(&node) = self.by_path.get(&path) {
                self.focused = self.row_of(node);
            }
        }
    }

    /// Open or close the folder at `row`. Focus inside a folder that closes
    /// moves to the folder.
    pub fn toggle(&mut self, row: usize) {
        let Some(&node) = self.row_nodes.get(row) else {
            return;
        };
        if self.nodes[node].kind != EntryKind::Folder {
            return;
        }
        if !self.expanded.remove(&node) {
            self.expanded.insert(node);
        }
        self.refresh_rows();
        if self.focused.is_none() {
            self.focused = self.row_of(node);
        }
    }

    /// Click, Enter or Space on `row`: open a file or toggle a folder.
    pub fn activate(&mut self, row: usize) -> Option<TreeAction> {
        let row_value = self.rows.get(row)?;
        self.focused = Some(row);
        match row_value.kind {
            EntryKind::File => Some(TreeAction::OpenFile {
                relative_path: row_value.relative_path.clone(),
            }),
            EntryKind::Folder => {
                self.toggle(row);
                None
            }
        }
    }

    /// Handle a key while the tree has focus.
    pub fn press(&mut self, key: TreeKey) -> Option<TreeAction> {
        let last = self.rows.len().checked_sub(1)?;
        let Some(current) = self.focused else {
            self.focused = Some(if key == TreeKey::End { last } else { 0 });
            return None;
        };
        let row = &self.rows[current];
        match key {
            TreeKey::Up => self.focused = Some(current.saturating_sub(1)),
            TreeKey::Down => self.focused = Some((current + 1).min(last)),
            TreeKey::Home => self.focused = Some(0),
            TreeKey::End => self.focused = Some(last),
            TreeKey::Right => match row.expanded {
                Some(false) => self.toggle(current),
                Some(true)
                    if self
                        .rows
                        .get(current + 1)
                        .is_some_and(|next| next.depth == row.depth + 1) =>
                {
                    self.focused = Some(current + 1);
                }
                _ => {}
            },
            TreeKey::Left => {
                if row.expanded == Some(true) {
                    self.toggle(current);
                } else if let Some(parent) = self.parents[self.row_nodes[current]] {
                    self.focused = self.row_of(parent);
                }
            }
            TreeKey::Activate => return self.activate(current),
        }
        None
    }

    /// Type-ahead: move focus to the next visible row whose name starts with
    /// the characters typed within [`TYPE_AHEAD_TIMEOUT`]. Typing one letter
    /// again cycles through the rows starting with it.
    pub fn type_ahead(&mut self, text: &str, at: Instant) {
        if text.chars().all(char::is_whitespace) || self.rows.is_empty() {
            return;
        }
        let stale = self
            .typed_at
            .is_none_or(|previous| at.saturating_duration_since(previous) > TYPE_AHEAD_TIMEOUT);
        if stale {
            self.typed.clear();
        }
        self.typed.extend(text.chars().flat_map(char::to_lowercase));
        self.typed_at = Some(at);
        let mut letters = self.typed.chars();
        let first = letters.next().unwrap_or_default();
        let repeated = letters.all(|letter| letter == first);
        let (needle, start) = match self.focused {
            Some(current) if repeated => (first.to_string(), current + 1),
            Some(current) => (self.typed.clone(), current),
            None => (self.typed.clone(), 0),
        };
        let count = self.rows.len();
        let found = (0..count)
            .map(|offset| (start + offset) % count)
            .find(|row| {
                self.rows[*row]
                    .name
                    .chars()
                    .flat_map(char::to_lowercase)
                    .collect::<String>()
                    .starts_with(&needle)
            });
        if let Some(row) = found {
            self.focused = Some(row);
        }
    }
}

impl Workspace {
    /// The workspace's file tree with every folder closed.
    pub fn file_tree(&self) -> Result<FileTree, AppError> {
        let entries = self
            .engine()
            .list_workspace_entries()
            .map_err(|error| AppError::from_core(error, self.root(), Access::ReadWorkspace))?;
        Ok(FileTree::new(entries.into_iter().map(|entry| TreeEntry {
            kind: match entry.kind {
                WorkspaceEntryKind::Folder => EntryKind::Folder,
                WorkspaceEntryKind::File => EntryKind::File,
            },
            relative_path: entry.relative_path,
            not_downloaded: entry.not_downloaded,
        })))
    }

    /// The file at `relative_path`, for the document area. Reads nothing
    /// and writes nothing. A file that cloud storage hasn't downloaded yet
    /// doesn't open, as in the legacy app.
    pub fn open_file(&self, relative_path: &str) -> Result<OpenedFile, AppError> {
        let path = self
            .engine()
            .resolve_managed_path(relative_path, "file_open")
            .map_err(|error| AppError::from_core(error, relative_path, Access::ReadFile))?;
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && !is_dataless(&metadata) => {}
            Ok(metadata) if metadata.is_file() => return Err(not_downloaded()),
            _ if has_icloud_placeholder(&path) => return Err(not_downloaded()),
            _ => {
                return Err(AppError::new(
                    "file_missing",
                    format!("{relative_path} isn't in the workspace anymore."),
                ));
            }
        }
        let file_name = relative_path.rsplit('/').next().unwrap_or(relative_path);
        Ok(OpenedFile {
            name: shown_name(file_name, EntryKind::File),
            relative_path: relative_path.to_owned(),
        })
    }
}

fn not_downloaded() -> AppError {
    AppError::new(
        "file_not_downloaded",
        "This file isn't downloaded yet. Try again once it is.",
    )
}

/// Whether iCloud keeps `.Name.icloud` in place of the missing `Name`, as
/// `local-core`'s scan lists it.
fn has_icloud_placeholder(path: &std::path::Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let placeholder = path.with_file_name(format!(".{name}.icloud"));
    std::fs::symlink_metadata(placeholder).is_ok_and(|metadata| metadata.is_file())
}

/// Whether macOS reports the file as dataless: its content lives only in
/// cloud storage. `local-core`'s scan marks these rows as not downloaded.
#[cfg(target_os = "macos")]
fn is_dataless(metadata: &std::fs::Metadata) -> bool {
    use std::os::macos::fs::MetadataExt;
    const SF_DATALESS: u32 = 0x4000_0000;
    metadata.st_flags() & SF_DATALESS != 0
}

#[cfg(not(target_os = "macos"))]
fn is_dataless(_metadata: &std::fs::Metadata) -> bool {
    false
}

pub(crate) fn parent_of(path: &str) -> Option<&str> {
    path.rsplit_once('/').map(|(parent, _)| parent)
}

/// Whether the item at `path` can move into `folder`: not its own folder,
/// where it already is, and never a folder into itself or a folder inside
/// it.
pub(crate) fn can_move_into(path: &str, folder: &str) -> bool {
    parent_of(path).unwrap_or("") != folder && remap(folder, path, path).is_none()
}

/// `path` after the item at `from` moved to `to`, or `None` when the move
/// didn't touch it.
pub(crate) fn remap(path: &str, from: &str, to: &str) -> Option<String> {
    if path == from {
        return Some(to.to_owned());
    }
    path.strip_prefix(from)
        .filter(|rest| rest.starts_with('/'))
        .map(|rest| format!("{to}{rest}"))
}

/// Markdown files show without `.md`; everything else shows whole.
pub(crate) fn shown_name(file_name: &str, kind: EntryKind) -> String {
    let markdown = kind == EntryKind::File
        && file_name.len() > 3
        && file_name.is_char_boundary(file_name.len() - 3)
        && file_name[file_name.len() - 3..].eq_ignore_ascii_case(".md");
    if markdown {
        file_name[..file_name.len() - 3].to_owned()
    } else {
        file_name.to_owned()
    }
}

/// A name folded for sorting: lowercase and without accents, so "Émile"
/// sorts with "emile", as the legacy app's base-sensitivity collator does.
fn sort_key(name: &str) -> String {
    name.nfd()
        .filter(|character| !is_combining_mark(*character))
        .flat_map(char::to_lowercase)
        .collect()
}

/// Compare folded names with digit runs compared as numbers, so "Note 2"
/// sorts before "Note 10". As in the root collation order the legacy
/// collator uses, punctuation and symbols come before digits, and digits
/// before letters.
fn natural_cmp(left: &str, right: &str) -> Ordering {
    let mut left = left.chars().peekable();
    let mut right = right.chars().peekable();
    loop {
        match (left.peek().copied(), right.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(a), Some(b)) if a.is_ascii_digit() && b.is_ascii_digit() => {
                let a = digits(&mut left);
                let b = digits(&mut right);
                let (a, b) = (a.trim_start_matches('0'), b.trim_start_matches('0'));
                let order = a.len().cmp(&b.len()).then_with(|| a.cmp(b));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(a), Some(b)) => {
                let order = group(a).cmp(&group(b)).then_with(|| a.cmp(&b));
                if order != Ordering::Equal {
                    return order;
                }
                left.next();
                right.next();
            }
        }
    }
}

/// A character's group in the root collation order: spaces, punctuation
/// and symbols first, then digits, then letters.
fn group(character: char) -> u8 {
    if character.is_numeric() {
        1
    } else if character.is_alphabetic() {
        2
    } else {
        0
    }
}

fn digits(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut run = String::new();
    while let Some(digit) = chars.next_if(char::is_ascii_digit) {
        run.push(digit);
    }
    run
}
