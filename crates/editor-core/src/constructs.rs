//! The Markdown constructs the editor draws in place, each with the syntax
//! it hides and whether that syntax shows now. The reveal rules are the
//! ones `editor-egui` draws with, so a test or an accessibility layer can
//! read what the editor shows without laying anything out.

use comrak::Arena;
use comrak::nodes::{AstNode, NodeValue};

use crate::doc::MdDoc;
use crate::offset_types::{Grapheme, RangeExt as _};

/// A kind of construct the editor draws in place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstructKind {
    /// An ATX heading, such as `## Title`.
    Heading {
        level: u8,
    },
    Emphasis,
    Strong,
    Strikethrough,
    InlineCode,
    Link,
    ListItem,
    TaskItem {
        checked: bool,
    },
    BlockQuote,
    /// A fenced code block.
    CodeBlock,
}

/// One construct in the note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Construct {
    pub kind: ConstructKind,
    /// The construct's source, syntax included.
    pub range: (Grapheme, Grapheme),
    /// The syntax the editor hides while the construct isn't revealed: a
    /// heading's `# `, emphasis markers, a link's brackets and destination,
    /// a list or task marker, a quote's `> `, or a code block's fences.
    pub syntax: Vec<(Grapheme, Grapheme)>,
    /// Whether the syntax shows, because the caret, the selection, a find
    /// match or a search preview reaches into the construct.
    pub revealed: bool,
}

impl MdDoc {
    /// Every construct the editor draws in place, in source order.
    pub fn constructs(&mut self) -> Vec<Construct> {
        let arena = Arena::new();
        let root = self.reparse(&arena);
        root.descendants()
            .filter_map(|node| self.construct(node))
            .collect()
    }

    fn construct<'a>(&self, node: &'a AstNode<'a>) -> Option<Construct> {
        let value = node.data.borrow().value.clone();
        let range = self.node_range(node);
        let edges = || {
            [self.head_range(node), self.tail_range(node)]
                .into_iter()
                .flatten()
                .filter(|range| !range.is_empty())
                .collect::<Vec<_>>()
        };
        let marker = || self.prefix_range(node).into_iter().collect::<Vec<_>>();
        let (kind, syntax, revealed) = match value {
            NodeValue::Heading(heading) if !heading.setext => (
                ConstructKind::Heading {
                    level: heading.level,
                },
                // The text after the `#`s starts the heading's content,
                // which has no child node when the heading is empty.
                self.prefix_range(node)
                    .or(Some(range))
                    .into_iter()
                    .collect(),
                self.range_revealed(self.node_first_line(node), true),
            ),
            // A setext heading's syntax is the `===` or `---` line under it.
            NodeValue::Heading(heading) => (
                ConstructKind::Heading {
                    level: heading.level,
                },
                vec![self.node_line(node, self.node_last_line(node))],
                self.reveal_setext_syntax(node),
            ),
            NodeValue::Emph => (ConstructKind::Emphasis, edges(), self.node_revealed(node)),
            NodeValue::Strong => (ConstructKind::Strong, edges(), self.node_revealed(node)),
            NodeValue::Strikethrough if !crate::nodes::is_single_tilde_strikethrough(node) => (
                ConstructKind::Strikethrough,
                edges(),
                self.node_revealed(node),
            ),
            NodeValue::Code(_) => (ConstructKind::InlineCode, edges(), self.node_revealed(node)),
            NodeValue::Link(_) => (ConstructKind::Link, edges(), self.node_revealed(node)),
            NodeValue::Item(_) => (ConstructKind::ListItem, marker(), self.reveal(node)),
            NodeValue::TaskItem(task) => (
                ConstructKind::TaskItem {
                    checked: task.symbol.is_some(),
                },
                marker(),
                self.reveal(node),
            ),
            NodeValue::BlockQuote => (ConstructKind::BlockQuote, marker(), self.reveal(node)),
            NodeValue::CodeBlock(code_block) if code_block.fenced => {
                let first = self.node_first_line(node);
                let last = self.node_last_line(node);
                let mut fences = vec![first];
                if last != first && self.is_closing_fence(node, &code_block, last) {
                    fences.push(last);
                }
                (
                    ConstructKind::CodeBlock,
                    fences,
                    self.reveal_fenced_code_block(node, &code_block),
                )
            }
            _ => return None,
        };
        Some(Construct {
            kind,
            range,
            syntax,
            revealed,
        })
    }
}
