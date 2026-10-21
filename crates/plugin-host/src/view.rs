//! A plugin's view as a tree, checked before anything draws it.
//!
//! A plugin returns a flat list of nodes whose `children` hold indices into
//! the list, with the root first. [`View::from_nodes`] turns that list into
//! a tree, or says which rule it breaks: an index out of range, a node
//! reached twice or from itself, a node where its kind can't go, a repeated
//! node ID, an unlabeled control, a table row whose cells don't match the
//! columns or hold no text, an empty menu, an input value of the wrong
//! form, a virtual list too long to draw, or a view too large or deep to
//! draw. [`check_rows`] does the same for the rows of a
//! virtual list.

use std::collections::HashSet;

use crate::{Choice, Column, Node, NodeKind, Value, VirtualRow};

/// The most nodes a view may hold.
pub const MAX_NODES: usize = 10_000;

/// The deepest a view's panels may nest.
pub const MAX_DEPTH: usize = 64;

/// The most rows a virtual list may hold. Noura places rows in `f32`
/// points, 28 apart, which stay within a point of where they belong up to
/// here.
pub const MAX_ROWS: u32 = 1_000_000;

/// A checked view: its root and everything under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    root: ViewNode,
}

/// One node of a checked view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewNode {
    /// Unique within the view. Events name the node by it.
    pub id: String,
    pub kind: ViewKind,
    /// The nodes a panel or a form lays out, a list's items, a table's
    /// rows or a menu's items. An item, a row or a button holds its menu
    /// here, if it has one.
    pub children: Vec<ViewNode>,
}

/// What a node draws. Every label holds more than spaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewKind {
    /// Its children, top to bottom, named for screen readers by `label`.
    Panel {
        label: Option<String>,
    },
    /// A line of plain text.
    Text(String),
    /// Markdown. Images show as their description and never load.
    Markdown(String),
    /// A button, or a menu button when it holds a menu.
    Button {
        label: String,
    },
    /// A list whose children are its items.
    List {
        label: String,
    },
    /// An item of a list, which may hold a menu.
    Item {
        label: String,
        detail: Option<String>,
    },
    /// A list of `rows` rows that come from the plugin as they scroll into
    /// view.
    VirtualList {
        label: String,
        rows: u32,
    },
    /// A table with at least one column, whose children are its rows.
    Table {
        label: String,
        columns: Vec<Column>,
    },
    /// A table row with one cell per column, which may hold a menu.
    Row {
        cells: Vec<String>,
    },
    /// Its children, top to bottom, then a submit button labeled `submit`.
    Form {
        label: String,
        submit: String,
    },
    /// An input. A date holds `YYYY-MM-DD` and a date and time RFC 3339
    /// with an offset; a choice input's value names one of its `choices`.
    Input {
        label: String,
        value: Value,
        choices: Vec<Choice>,
    },
    /// A menu of an item, a row or a button, holding at least one item.
    Menu {
        label: String,
    },
    MenuItem {
        label: String,
        enabled: bool,
    },
    /// A line between menu items.
    Separator,
}

/// Why a node list or a virtual list's rows can't be drawn. The message
/// names the rule and the node, for the error shown in place of the view.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ViewError {
    #[error("The plugin returned no nodes.")]
    Empty,
    #[error("The plugin returned {count} nodes; a view holds at most {MAX_NODES}.")]
    TooManyNodes { count: usize },
    #[error("Node {node} lists child {child}, but the view has {count} nodes.")]
    OutOfRange {
        node: String,
        child: u32,
        count: usize,
    },
    #[error("The view reaches node {node} twice, through a cycle or a shared node.")]
    Cycle { node: String },
    #[error("The {kind} {node} holds no other nodes.")]
    ChildrenOfLeaf { kind: &'static str, node: String },
    #[error("Two nodes have the ID {id}. Each node needs its own ID.")]
    DuplicateId { id: String },
    #[error("The {kind} {node} has no label.")]
    Unlabeled { kind: &'static str, node: String },
    #[error("The view nests deeper than {MAX_DEPTH} panels.")]
    TooDeep,
    /// A node sits where its kind can't go, or holds a node that can't go
    /// in it. The message says which.
    #[error("{0}")]
    Misplaced(String),
    #[error("The row {node} has {cells} cells, but its table has {columns} columns.")]
    RowCells {
        node: String,
        cells: usize,
        columns: usize,
    },
    #[error("The table {node} has no columns.")]
    NoColumns { node: String },
    #[error("The menu {node} has no items.")]
    EmptyMenu { node: String },
    /// An input's value, options or a table's columns break a rule. The
    /// message says which.
    #[error("{0}")]
    Invalid(String),
    #[error("The plugin returned {returned} rows when Noura asked for {asked}.")]
    TooManyRows { returned: usize, asked: u32 },
    #[error("A row has no ID.")]
    RowWithoutId,
    #[error("The virtual list {node} holds {rows} rows; a virtual list holds at most {MAX_ROWS}.")]
    TooLong { node: String, rows: u32 },
    #[error("Two rows have the ID {id}. Each row needs its own ID.")]
    DuplicateRowId { id: String },
}

impl View {
    /// Check a plugin's node list and build the tree it describes. Nodes
    /// that the root doesn't reach are left out.
    pub fn from_nodes(nodes: Vec<Node>) -> Result<Self, ViewError> {
        if nodes.is_empty() {
            return Err(ViewError::Empty);
        }
        if nodes.len() > MAX_NODES {
            return Err(ViewError::TooManyNodes { count: nodes.len() });
        }
        let mut ids = HashSet::with_capacity(nodes.len());
        for node in &nodes {
            if !ids.insert(node.id.as_str()) {
                return Err(ViewError::DuplicateId {
                    id: node.id.clone(),
                });
            }
            for &child in &node.children {
                if child as usize >= nodes.len() {
                    return Err(ViewError::OutOfRange {
                        node: node.id.clone(),
                        child,
                        count: nodes.len(),
                    });
                }
            }
        }
        for node in &nodes {
            check_node(node)?;
            check_children(node, &nodes)?;
        }
        check_place(&nodes[0], None)?;
        let mut seen = vec![false; nodes.len()];
        let root = build(&nodes, 0, &mut seen, 0, false)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &ViewNode {
        &self.root
    }
}

/// Check the rows a plugin returned when Noura asked for `asked` rows of a
/// virtual list.
pub fn check_rows(rows: Vec<VirtualRow>, asked: u32) -> Result<Vec<VirtualRow>, ViewError> {
    if rows.len() > asked as usize {
        return Err(ViewError::TooManyRows {
            returned: rows.len(),
            asked,
        });
    }
    let mut ids = HashSet::with_capacity(rows.len());
    for row in &rows {
        if blank(&row.id) {
            return Err(ViewError::RowWithoutId);
        }
        if blank(&row.item.label) {
            return Err(ViewError::Unlabeled {
                kind: "row",
                node: row.id.clone(),
            });
        }
        if !ids.insert(row.id.as_str()) {
            return Err(ViewError::DuplicateRowId { id: row.id.clone() });
        }
    }
    Ok(rows)
}

fn blank(text: &str) -> bool {
    text.trim().is_empty()
}

/// The kind's name, as messages say it.
fn kind_name(kind: &NodeKind) -> &'static str {
    match kind {
        NodeKind::Panel(_) => "panel",
        NodeKind::Text(_) => "text",
        NodeKind::Markdown(_) => "Markdown",
        NodeKind::Button(_) => "button",
        NodeKind::List(_) => "list",
        NodeKind::Item(_) => "item",
        NodeKind::VirtualList(_) => "virtual list",
        NodeKind::Table(_) => "table",
        NodeKind::Row(_) => "row",
        NodeKind::Form(_) => "form",
        NodeKind::Input(_) => "input",
        NodeKind::Menu(_) => "menu",
        NodeKind::MenuItem(_) => "menu item",
        NodeKind::Separator => "separator",
    }
}

/// The rules a node keeps on its own: labels, a row's text, a virtual
/// list's length, a table's columns and an input's value and options.
fn check_node(node: &Node) -> Result<(), ViewError> {
    let unlabeled = || ViewError::Unlabeled {
        kind: kind_name(&node.kind),
        node: node.id.clone(),
    };
    let invalid = |message: String| Err(ViewError::Invalid(message));
    let id = &node.id;
    match &node.kind {
        NodeKind::Panel(_) | NodeKind::Text(_) | NodeKind::Markdown(_) | NodeKind::Separator => {
            Ok(())
        }
        // Screen readers name a row by its first cell with text.
        NodeKind::Row(cells) if cells.iter().all(|cell| blank(cell)) => {
            invalid(format!("The row {id} has no cell with text."))
        }
        NodeKind::Row(_) => Ok(()),
        NodeKind::Button(label) | NodeKind::List(label) | NodeKind::Menu(label) => {
            if blank(label) {
                Err(unlabeled())
            } else {
                Ok(())
            }
        }
        NodeKind::Item(item) if blank(&item.label) => Err(unlabeled()),
        NodeKind::VirtualList(list) if blank(&list.label) => Err(unlabeled()),
        NodeKind::VirtualList(list) if list.rows > MAX_ROWS => Err(ViewError::TooLong {
            node: id.clone(),
            rows: list.rows,
        }),
        NodeKind::MenuItem(item) if blank(&item.label) => Err(unlabeled()),
        NodeKind::Item(_) | NodeKind::VirtualList(_) | NodeKind::MenuItem(_) => Ok(()),
        NodeKind::Form(form) if blank(&form.label) => Err(unlabeled()),
        NodeKind::Form(form) if blank(&form.submit) => {
            invalid(format!("The form {id} has no submit label."))
        }
        NodeKind::Form(_) => Ok(()),
        NodeKind::Table(table) => {
            if blank(&table.label) {
                return Err(unlabeled());
            }
            if table.columns.is_empty() {
                return Err(ViewError::NoColumns { node: id.clone() });
            }
            let mut columns = HashSet::new();
            for column in &table.columns {
                if blank(&column.id) {
                    return invalid(format!("The table {id} has a column without an ID."));
                }
                if blank(&column.title) {
                    return invalid(format!(
                        "The column {} of the table {id} has no title.",
                        column.id
                    ));
                }
                if !columns.insert(column.id.as_str()) {
                    return invalid(format!(
                        "The table {id} has two columns with the ID {}.",
                        column.id
                    ));
                }
            }
            Ok(())
        }
        NodeKind::Input(input) => {
            if blank(&input.label) {
                return Err(unlabeled());
            }
            check_value(id, &input.value, &input.choices)
        }
    }
}

/// An input's value has the form its kind needs, and only a choice input
/// has options, one of which its value names.
fn check_value(id: &str, value: &Value, choices: &[Choice]) -> Result<(), ViewError> {
    let invalid = |message: String| Err(ViewError::Invalid(message));
    if !matches!(value, Value::Choice(_)) && !choices.is_empty() {
        return invalid(format!(
            "Only a choice input has options, and the input {id} isn't one."
        ));
    }
    match value {
        Value::Date(Some(date)) if !workspace_format::is_civil_date(date) => invalid(format!(
            "The input {id} holds {date}, which isn't a date in the form YYYY-MM-DD."
        )),
        Value::DateTime(Some(at)) if !workspace_format::is_instant(at) => invalid(format!(
            "The input {id} holds {at}, which isn't an RFC 3339 date and time with an offset."
        )),
        Value::Choice(chosen) => {
            if choices.is_empty() {
                return invalid(format!("The choice input {id} has no options."));
            }
            let mut ids = HashSet::new();
            for choice in choices {
                if blank(&choice.id) {
                    return invalid(format!("The input {id} has an option without an ID."));
                }
                if blank(&choice.label) {
                    return invalid(format!(
                        "The option {} of the input {id} has no label.",
                        choice.id
                    ));
                }
                if !ids.insert(choice.id.as_str()) {
                    return invalid(format!(
                        "The input {id} has two options with the ID {}.",
                        choice.id
                    ));
                }
            }
            match chosen {
                Some(chosen) if !ids.contains(chosen.as_str()) => invalid(format!(
                    "The input {id} chooses {chosen}, which isn't one of its options."
                )),
                _ => Ok(()),
            }
        }
        _ => Ok(()),
    }
}

/// Where a node of each kind may sit. `parent` is `None` for the root.
fn check_place(node: &Node, parent: Option<&Node>) -> Result<(), ViewError> {
    let misplaced = |message: String| Err(ViewError::Misplaced(message));
    let id = &node.id;
    let parent_kind = parent.map(|parent| &parent.kind);
    match (&node.kind, parent_kind) {
        (NodeKind::Item(_), Some(NodeKind::List(_))) => Ok(()),
        (NodeKind::Item(_), _) => misplaced(format!("The item {id} isn't in a list.")),
        (NodeKind::Row(_), Some(NodeKind::Table(_))) => Ok(()),
        (NodeKind::Row(_), _) => misplaced(format!("The row {id} isn't in a table.")),
        (NodeKind::Menu(_), Some(NodeKind::Item(_) | NodeKind::Row(_) | NodeKind::Button(_))) => {
            Ok(())
        }
        (NodeKind::Menu(_), _) => misplaced(format!(
            "The menu {id} isn't on an item, a row or a button."
        )),
        (NodeKind::MenuItem(_) | NodeKind::Separator, Some(NodeKind::Menu(_))) => Ok(()),
        (NodeKind::MenuItem(_) | NodeKind::Separator, _) => misplaced(format!(
            "The {} {id} isn't in a menu.",
            kind_name(&node.kind)
        )),
        _ => Ok(()),
    }
}

/// The nodes `parent` may hold: blocks in a panel or a form, items in a
/// list, rows in a table with one cell per column, items and separators
/// in a menu, and at most one menu on an item, a row or a button.
fn check_children(parent: &Node, nodes: &[Node]) -> Result<(), ViewError> {
    let id = &parent.id;
    let kind = kind_name(&parent.kind);
    let children = parent.children.iter().map(|&child| &nodes[child as usize]);
    let misplaced = |message: String| Err(ViewError::Misplaced(message));
    match &parent.kind {
        NodeKind::Panel(_) | NodeKind::Form(_) => {
            for child in children {
                check_place(child, Some(parent))?;
            }
            Ok(())
        }
        NodeKind::List(_) => {
            for child in children {
                if !matches!(child.kind, NodeKind::Item(_)) {
                    return misplaced(format!(
                        "The list {id} holds {}, which isn't an item.",
                        child.id
                    ));
                }
            }
            Ok(())
        }
        NodeKind::Table(table) => {
            for child in children {
                let NodeKind::Row(cells) = &child.kind else {
                    return misplaced(format!(
                        "The table {id} holds {}, which isn't a row.",
                        child.id
                    ));
                };
                if cells.len() != table.columns.len() {
                    return Err(ViewError::RowCells {
                        node: child.id.clone(),
                        cells: cells.len(),
                        columns: table.columns.len(),
                    });
                }
            }
            Ok(())
        }
        NodeKind::Item(_) | NodeKind::Row(_) | NodeKind::Button(_) => {
            if parent.children.len() > 1 {
                return misplaced(format!("The {kind} {id} holds more than one menu."));
            }
            for child in children {
                if !matches!(child.kind, NodeKind::Menu(_)) {
                    return misplaced(format!(
                        "The {kind} {id} holds {}, which isn't a menu.",
                        child.id
                    ));
                }
            }
            Ok(())
        }
        NodeKind::Menu(_) => {
            let mut items = 0;
            for child in children {
                match child.kind {
                    NodeKind::MenuItem(_) => items += 1,
                    NodeKind::Separator => {}
                    _ => {
                        return misplaced(format!(
                            "The menu {id} holds {}, which isn't a menu item or a separator.",
                            child.id
                        ));
                    }
                }
            }
            if items == 0 {
                return Err(ViewError::EmptyMenu { node: id.clone() });
            }
            Ok(())
        }
        NodeKind::Text(_)
        | NodeKind::Markdown(_)
        | NodeKind::VirtualList(_)
        | NodeKind::Input(_)
        | NodeKind::MenuItem(_)
        | NodeKind::Separator => {
            if parent.children.is_empty() {
                Ok(())
            } else {
                Err(ViewError::ChildrenOfLeaf {
                    kind,
                    node: id.clone(),
                })
            }
        }
    }
}

/// The tree under `nodes[index]`, marking each node it reaches. `in_form`
/// says a form holds the node.
fn build(
    nodes: &[Node],
    index: usize,
    seen: &mut [bool],
    depth: usize,
    in_form: bool,
) -> Result<ViewNode, ViewError> {
    let node = &nodes[index];
    if seen[index] {
        return Err(ViewError::Cycle {
            node: node.id.clone(),
        });
    }
    seen[index] = true;
    if depth > MAX_DEPTH {
        return Err(ViewError::TooDeep);
    }
    let is_form = matches!(node.kind, NodeKind::Form(_));
    if is_form && in_form {
        return Err(ViewError::Misplaced(format!(
            "The form {} sits inside another form.",
            node.id
        )));
    }
    let kind = match &node.kind {
        NodeKind::Panel(label) => ViewKind::Panel {
            label: label.clone().filter(|label| !blank(label)),
        },
        NodeKind::Text(text) => ViewKind::Text(text.clone()),
        NodeKind::Markdown(markdown) => ViewKind::Markdown(markdown.clone()),
        NodeKind::Button(label) => ViewKind::Button {
            label: label.clone(),
        },
        NodeKind::List(label) => ViewKind::List {
            label: label.clone(),
        },
        NodeKind::Item(item) => ViewKind::Item {
            label: item.label.clone(),
            detail: item.detail.clone().filter(|detail| !blank(detail)),
        },
        NodeKind::VirtualList(list) => ViewKind::VirtualList {
            label: list.label.clone(),
            rows: list.rows,
        },
        NodeKind::Table(table) => ViewKind::Table {
            label: table.label.clone(),
            columns: table.columns.clone(),
        },
        NodeKind::Row(cells) => ViewKind::Row {
            cells: cells.clone(),
        },
        NodeKind::Form(form) => ViewKind::Form {
            label: form.label.clone(),
            submit: form.submit.clone(),
        },
        NodeKind::Input(input) => ViewKind::Input {
            label: input.label.clone(),
            value: input.value.clone(),
            choices: input.choices.clone(),
        },
        NodeKind::Menu(label) => ViewKind::Menu {
            label: label.clone(),
        },
        NodeKind::MenuItem(item) => ViewKind::MenuItem {
            label: item.label.clone(),
            enabled: item.enabled,
        },
        NodeKind::Separator => ViewKind::Separator,
    };
    let children = node
        .children
        .iter()
        .map(|&child| build(nodes, child as usize, seen, depth + 1, in_form || is_form))
        .collect::<Result<_, _>>()?;
    Ok(ViewNode {
        id: node.id.clone(),
        kind,
        children,
    })
}
