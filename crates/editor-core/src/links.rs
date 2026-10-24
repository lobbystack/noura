//! Links in the note and the lookups that resolve them through the
//! embedding app's [`LinkResolver`](crate::resolvers::LinkResolver).

use comrak::Arena;
use comrak::nodes::{AstNode, NodeValue};

use crate::bounds::RangesExt as _;
use crate::doc::MdDoc;
use crate::offset_types::{Grapheme, RangeExt as _};
use crate::operation_types::Operation;
use crate::resolvers::{LinkState, ResolvedLink};

/// How far before the caret the `[[` of a link being typed may start, in
/// graphemes.
const LINK_QUERY_REACH: usize = 200;

/// The `[[` link being typed at the caret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkQuery {
    /// What follows `[[` up to the caret.
    pub typed: String,
    /// Where `typed` sits: from just after `[[` to the caret.
    pub range: (Grapheme, Grapheme),
}

/// Whether `grapheme` ends the target of a `[[` link: a `#heading`, a
/// `|display text`, a bracket or a new line.
fn ends_link_target(grapheme: &str) -> bool {
    matches!(grapheme, "[" | "]" | "|" | "#" | "\n" | "\r" | "\r\n")
}

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

    /// The `[[` link being typed at the caret, in text that takes links:
    /// the caret follows `[[` and what's typed after it, with no `#`, `|`,
    /// bracket or new line between them. `None` with a selection, in code,
    /// in a read-only note and in plain text.
    pub fn link_query(&mut self) -> Option<LinkQuery> {
        self.with_ast(|doc, root| doc.link_query_in(root))
    }

    /// [`Self::link_query`] in the parse `root` of the current text.
    pub(crate) fn link_query_in(&self, root: &'ast AstNode<'ast>) -> Option<LinkQuery> {
        let (anchor, caret) = self.buffer.current.selection;
        if self.plaintext || self.readonly || anchor != caret {
            return None;
        }
        let grapheme = |at: usize| &self.buffer[(Grapheme(at), Grapheme(at + 1))];
        let mut at = caret.0;
        let start = loop {
            if at == 0 || caret.0 - at > LINK_QUERY_REACH {
                return None;
            }
            at -= 1;
            match grapheme(at) {
                "[" if at > 0 && grapheme(at - 1) == "[" => break Grapheme(at + 1),
                found if ends_link_target(found) => return None,
                _ => {}
            }
        };
        let paragraphs = self
            .bounds
            .inline_paragraphs
            .find_containing(caret, true, true);
        if paragraphs.is_empty() {
            return None;
        }
        let in_code = root.descendants().any(|node| {
            matches!(node.data.borrow().value, NodeValue::Code(_))
                && self.node_range(node).contains(start, false, false)
        });
        if in_code {
            return None;
        }
        Some(LinkQuery {
            typed: self.buffer[(start, caret)].to_owned(),
            range: (start, caret),
        })
    }

    /// The edit that puts `link` in place of the `[[` link query in
    /// `root`: inside a link that's already closed, it replaces the whole
    /// target and keeps the `]]`, a `|display text` or a `#heading` after
    /// it; otherwise it replaces what's typed and adds `]]`.
    pub(crate) fn complete_link_event(
        &self,
        root: &'ast AstNode<'ast>,
        link: &str,
    ) -> Option<crate::input::Event> {
        use crate::input::{Event, Location, Region};
        let LinkQuery {
            range: (start, caret),
            ..
        } = self.link_query_in(root)?;
        let last = self.last_cursor_position();
        let mut end = caret;
        while end < last && !ends_link_target(&self.buffer[(end, end + 1)]) {
            end += 1;
        }
        let follows = |text: &str| {
            let after = end + text.chars().count();
            after <= last && &self.buffer[(end, after)] == text
        };
        let (replaced_end, text) = if follows("]]") {
            (end + 2, format!("{link}]]"))
        } else if follows("|") || follows("#") {
            (end, link.to_owned())
        } else {
            (caret, format!("{link}]]"))
        };
        Some(Event::Replace {
            region: Region::BetweenLocations {
                start: Location::Grapheme(start),
                end: Location::Grapheme(replaced_end),
            },
            text,
            advance_cursor: true,
        })
    }

    /// URL (or wikilink target) of the first link-like node whose source range
    /// intersects the current selection.
    pub fn selection_open_target(&mut self) -> Option<String> {
        let arena = Arena::new();
        let root = self.reparse(&arena);
        self.selected_link(root).map(|(url, _)| url)
    }

    /// [`Self::selection_open_target`] in the parse `root`, with whether
    /// the link is a wikilink.
    pub(crate) fn selected_link(&self, root: &'ast AstNode<'ast>) -> Option<(String, bool)> {
        let selection = self.buffer.current.selection;
        for node in root.descendants() {
            let (url, wikilink) = match &node.data.borrow().value {
                NodeValue::Link(l) | NodeValue::Image(l) => (l.url.clone(), false),
                NodeValue::WikiLink(nwl) => (nwl.url.clone(), true),
                _ => continue,
            };
            if !url.is_empty() && self.node_range(node).intersects(&selection, true) {
                return Some((url, wikilink));
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
