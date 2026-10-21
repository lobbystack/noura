//! The Markdown constructs the editor draws in place, each with the syntax
//! it hides and whether that syntax shows now. The reveal rules are the
//! ones `editor-egui` draws with, so a test or an accessibility layer can
//! read what the editor shows without laying anything out.

use comrak::Arena;
use comrak::nodes::{AstNode, NodeValue};

use crate::doc::MdDoc;
use crate::offset_types::{Grapheme, RangeExt as _};
use crate::spans::SpanKind;

/// A kind of construct the editor draws in place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstructKind {
    /// A heading: `## Title`, or a line with `===` or `---` under it.
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
    /// A quote that starts with `[!type]`, drawn labeled with its type.
    Callout,
    /// A fenced code block.
    CodeBlock,
    /// `==text==`, drawn highlighted.
    Highlight,
    /// `%%text%%`, hidden away from the caret.
    Comment,
    /// `#tag`, drawn styled.
    Tag,
    /// `^block-id` at the end of a line, drawn styled.
    BlockId,
    /// `<u>text</u>`, drawn underlined.
    Underline,
}

/// One construct in the note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Construct {
    pub kind: ConstructKind,
    /// The construct's source, syntax included.
    pub range: (Grapheme, Grapheme),
    /// The syntax the editor hides while the construct isn't revealed: a
    /// heading's `# `, emphasis markers, a link's brackets and destination,
    /// a list or task marker, a quote's `> `, a callout's `>` markers and
    /// `[!type]`, or a code block's fences.
    pub syntax: Vec<(Grapheme, Grapheme)>,
    /// What the editor draws in place of the hidden syntax, such as a
    /// callout's type.
    pub label: Option<String>,
    /// Whether the syntax shows, because the caret, the selection, a find
    /// match or a search preview reaches into the construct.
    pub revealed: bool,
}

impl MdDoc {
    /// Every construct the editor draws in place, in source order.
    pub fn constructs(&mut self) -> Vec<Construct> {
        let arena = Arena::new();
        let root = self.reparse(&arena);
        let mut constructs: Vec<Construct> = root
            .descendants()
            .filter_map(|node| self.construct(node))
            .collect();
        constructs.extend(self.bounds.spans.iter().filter_map(|span| {
            let kind = match span.kind {
                SpanKind::Comment => ConstructKind::Comment,
                SpanKind::Tag => ConstructKind::Tag,
                SpanKind::BlockId => ConstructKind::BlockId,
                SpanKind::Underline => ConstructKind::Underline,
                // A callout's construct covers its marker and title.
                SpanKind::CalloutMarker { .. } | SpanKind::CalloutTitle { .. } => return None,
            };
            Some(Construct {
                kind,
                range: span.range,
                syntax: span.syntax.clone(),
                label: None,
                revealed: self.span_revealed(span),
            })
        }));
        // Stable, so a construct and one nested at its start keep their order.
        constructs.sort_by_key(|construct| construct.range.start());
        constructs
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
        let mut label = None;
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
            NodeValue::Highlight => (ConstructKind::Highlight, edges(), self.node_revealed(node)),
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
            NodeValue::BlockQuote => match self.callout(node) {
                Some(callout) => {
                    label = Some(callout.label());
                    (
                        ConstructKind::Callout,
                        self.callout_syntax(node, &callout),
                        self.callout_revealed(node),
                    )
                }
                None => (ConstructKind::BlockQuote, marker(), self.reveal(node)),
            },
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
            label,
            revealed,
        })
    }
}
