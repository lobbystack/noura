//! A plugin's view as a tree, checked before anything draws it.
//!
//! A plugin returns a flat list of nodes whose `children` hold indices into
//! the list, with the root first. [`View::from_nodes`] turns that list into
//! a tree, or says which rule it breaks: an index out of range, a node
//! reached twice or from itself, children on a node that isn't a panel, a
//! repeated node ID, an unlabeled control, or a view too large or deep to
//! draw.

use std::collections::HashSet;

use crate::{Node, NodeKind};

/// The most nodes a view may hold.
pub const MAX_NODES: usize = 10_000;

/// The deepest a view's panels may nest.
pub const MAX_DEPTH: usize = 64;

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
    /// Empty unless the node is a panel.
    pub children: Vec<ViewNode>,
}

/// What a node draws.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewKind {
    /// Its children, top to bottom, named for screen readers by `label`.
    Panel { label: Option<String> },
    /// A line of plain text.
    Text(String),
    /// Markdown. Images show as their description and never load.
    Markdown(String),
    /// A button with a label that isn't blank.
    Button { label: String },
}

/// Why a node list can't be drawn. The message names the rule and the
/// node, for the error shown in place of the view.
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
    #[error("Node {node} is reached twice, so the view has a cycle or a shared node.")]
    Cycle { node: String },
    #[error("Only panels hold other nodes, and node {node} isn't a panel.")]
    ChildrenOfLeaf { node: String },
    #[error("Two nodes have the ID {id}. Each node needs its own ID.")]
    DuplicateId { id: String },
    #[error("The button {node} has no label.")]
    Unlabeled { node: String },
    #[error("The view nests deeper than {MAX_DEPTH} panels.")]
    TooDeep,
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
            match &node.kind {
                NodeKind::Panel(_) => {}
                _ if !node.children.is_empty() => {
                    return Err(ViewError::ChildrenOfLeaf {
                        node: node.id.clone(),
                    });
                }
                NodeKind::Button(label) if label.trim().is_empty() => {
                    return Err(ViewError::Unlabeled {
                        node: node.id.clone(),
                    });
                }
                _ => {}
            }
        }
        let mut seen = vec![false; nodes.len()];
        let root = build(&nodes, 0, &mut seen, 0)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &ViewNode {
        &self.root
    }
}

/// The tree under `nodes[index]`, marking each node it reaches.
fn build(
    nodes: &[Node],
    index: usize,
    seen: &mut [bool],
    depth: usize,
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
    let kind = match &node.kind {
        NodeKind::Panel(label) => ViewKind::Panel {
            label: label.clone().filter(|label| !label.trim().is_empty()),
        },
        NodeKind::Text(text) => ViewKind::Text(text.clone()),
        NodeKind::Markdown(markdown) => ViewKind::Markdown(markdown.clone()),
        NodeKind::Button(label) => ViewKind::Button {
            label: label.clone(),
        },
    };
    let children = node
        .children
        .iter()
        .map(|&child| build(nodes, child as usize, seen, depth + 1))
        .collect::<Result<_, _>>()?;
    Ok(ViewNode {
        id: node.id.clone(),
        kind,
        children,
    })
}
