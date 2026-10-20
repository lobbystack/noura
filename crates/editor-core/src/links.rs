//! Links in the note and the lookups that resolve them through the
//! embedding app's [`LinkResolver`](crate::resolvers::LinkResolver).

use comrak::Arena;
use comrak::nodes::{AstNode, NodeValue};

use crate::doc::MdDoc;
use crate::offset_types::{Grapheme, RangeExt as _};
use crate::operation_types::Operation;
use crate::resolvers::{LinkState, ResolvedLink};

pub fn node_link_url(node: &AstNode<'_>) -> String {
    match &node.data.borrow().value {
        NodeValue::Link(link) | NodeValue::Image(link) => link.url.clone(),
        _ => String::new(),
    }
}

impl<'ast> MdDoc {
    pub fn link_is_auto(&self, node: &'ast AstNode<'ast>, url: &str) -> bool {
        self.infix_range(node)
            .is_some_and(|r| &self.buffer[r] == url)
    }

    pub fn resolve_link(&self, url: &str) -> Option<ResolvedLink> {
        self.link_resolver.resolve_link(url)
    }

    pub fn link_state_for_url(&self, url: &str) -> LinkState {
        self.link_resolver.link_state(url)
    }

    pub fn link_state_for_wikilink(&self, url: &str) -> LinkState {
        self.link_resolver.wikilink_state(url)
    }

    /// URL (or wikilink target) of the first link-like node whose source range
    /// intersects the current selection.
    pub fn selection_open_target(&mut self) -> Option<String> {
        let arena = Arena::new();
        let root = self.reparse(&arena);
        let selection = self.buffer.current.selection;
        for node in root.descendants() {
            let url = match &node.data.borrow().value {
                NodeValue::Link(l) | NodeValue::Image(l) => l.url.clone(),
                NodeValue::WikiLink(nwl) => nwl.url.clone(),
                _ => continue,
            };
            if !url.is_empty() && self.node_range(node).intersects(&selection, true) {
                return Some(url);
            }
        }
        None
    }

    /// The selection that edits a link-like node's hidden part: an image's
    /// or labeled link's destination, a wikilink's interior, a bare
    /// autolink's whole URL. `true` if acting on it should force-reveal via
    /// `entered_atom` — a bare autolink *is* its URL, so there is no
    /// interior sub-range whose selection would reveal the source.
    pub fn atom_edit_selection(&self, node: &'ast AstNode<'ast>) -> ((Grapheme, Grapheme), bool) {
        let node_range = self.node_range(node);
        let is_image = matches!(node.data.borrow().value, NodeValue::Image(_));
        match &node.data.borrow().value {
            NodeValue::WikiLink(_) => ((node_range.start() + 2, node_range.end() - 2), false),
            NodeValue::Link(_) | NodeValue::Image(_) => {
                let url = node_link_url(node);
                if !is_image && !url.is_empty() && self.link_is_auto(node, &url) {
                    (node_range, true)
                } else {
                    // `[title](url)` / `![alt](url)` — the postfix is
                    // `](url)`; without a label there are no children (no
                    // postfix) and the url starts after the opening syntax.
                    let open_len = if is_image { 4 } else { 3 };
                    let url_range = match self.postfix_range(node) {
                        Some(postfix) => (postfix.start() + 2, postfix.end() - 1),
                        None => (node_range.start() + open_len, node_range.end() - 1),
                    };
                    if url_range.0 <= url_range.1 {
                        (url_range, false)
                    } else {
                        // degenerate: interior caret
                        let caret = node_range.start() + (open_len - 2);
                        ((caret, caret), false)
                    }
                }
            }
            _ => (node_range, false),
        }
    }

    /// With the whole of a link selected, select the part that edits it
    /// (see [`Self::atom_edit_selection`]) — the touch path into a link.
    pub fn enter_at_link(
        &mut self,
        root: &'ast AstNode<'ast>,
        operations: &mut Vec<Operation>,
    ) -> bool {
        let selection = self.buffer.current.selection;
        for node in root.descendants() {
            if !matches!(
                node.data.borrow().value,
                NodeValue::Link(_) | NodeValue::WikiLink(_)
            ) {
                continue;
            }
            let node_range = self.node_range(node);
            if selection != node_range {
                continue;
            }
            let (select, force_reveal) = self.atom_edit_selection(node);
            if force_reveal {
                self.entered_atom = Some(node_range);
            }
            operations.push(Operation::Select(select));
            return true;
        }
        false
    }
}

impl MdDoc {
    pub fn resolve_wikilink(&self, url: &str) -> Option<String> {
        self.link_resolver.resolve_wikilink(url)
    }
}
