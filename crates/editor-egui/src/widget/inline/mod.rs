use comrak::nodes::{AstNode, NodeFootnoteReference, NodeValue};
use editor_core::offset_types::{Grapheme, IntoRangeExt, RangeExt as _};

use crate::MdRender;

pub(crate) mod code;
pub(crate) mod emph;
pub(crate) mod escaped;
pub(crate) mod escaped_tag;
pub(crate) mod footnote_reference;
pub(crate) mod highlight;
pub(crate) mod html_inline;
pub(crate) mod image;
pub(crate) mod line_break;
pub(crate) mod link;
pub(crate) mod math;
pub(crate) mod short_code;
pub(crate) mod soft_break;
pub(crate) mod spoilered_text;
pub(crate) mod strikethrough;
pub(crate) mod strong;
pub(crate) mod subscript;
pub(crate) mod superscript;
pub(crate) mod text;
pub(crate) mod underline;
pub(crate) mod wiki_link;

#[derive(Default, Debug)]
pub struct Response {
    pub clicked: bool,
    pub hovered: bool,
}

impl std::ops::BitOrAssign for Response {
    fn bitor_assign(&mut self, rhs: Self) {
        self.clicked |= rhs.clicked;
        self.hovered |= rhs.hovered;
    }
}

// ─── inline dispatcher + circumfix helper ───────────────────────────

use crate::widget::utils::wrap_layout::{Format, Layout, StyleInfo};

impl<'ast> MdRender {
    /// Emit `InlineItem`s for one inline AST node into `layout`.
    /// Dispatches on the node's `NodeValue` to a per-kind `layout_X`.
    pub fn layout_inline(
        &self,
        layout: &mut Layout,
        node: &'ast AstNode<'ast>,
        range: (Grapheme, Grapheme),
    ) {
        // Syntax inside a hidden comment, and hidden `<u>` tags, draw
        // nothing. Text splits itself around them in `layout_text`.
        if !matches!(node.data.borrow().value, NodeValue::Text(_)) {
            let node_range = self.node_range(node);
            let hidden = self.spans_in(node_range).find_map(|span| {
                span.syntax
                    .iter()
                    .find(|syntax| syntax.contains_range(&node_range, true, true))
                    .filter(|_| !self.span_revealed(span))
                    .copied()
            });
            if let Some(syntax) = hidden {
                if node_range.start() == syntax.start() && !syntax.trim(&range).is_empty() {
                    layout.push_override(syntax.trim(&range), "", self.text_format_syntax());
                }
                return;
            }
        }
        match &node.data.borrow().value {
            NodeValue::FrontMatter(_) => {}
            NodeValue::Raw(_) => unreachable!("can only be created programmatically"),

            NodeValue::Alert(_)
            | NodeValue::BlockQuote
            | NodeValue::DescriptionItem(_)
            | NodeValue::DescriptionList
            | NodeValue::Document
            | NodeValue::FootnoteDefinition(_)
            | NodeValue::Item(_)
            | NodeValue::List(_)
            | NodeValue::MultilineBlockQuote(_)
            | NodeValue::Table(_)
            | NodeValue::TableRow(_)
            | NodeValue::TaskItem(_)
            | NodeValue::CodeBlock(_)
            | NodeValue::DescriptionDetails
            | NodeValue::DescriptionTerm
            | NodeValue::Heading(_)
            | NodeValue::HtmlBlock(_)
            | NodeValue::Paragraph
            | NodeValue::TableCell
            | NodeValue::ThematicBreak => {
                unimplemented!("not an inline: {:?}", node.data.borrow().value)
            }
            NodeValue::Subtext => unimplemented!("extension disabled"),

            NodeValue::Code(_) => self.layout_code(layout, node, range),
            NodeValue::Emph => self.layout_emph(layout, node, range),
            NodeValue::Escaped => self.layout_escaped(layout, node, range),
            NodeValue::EscapedTag(_) => self.layout_escaped_tag(layout, node, range),
            NodeValue::FootnoteReference(node_footnote_reference) => {
                let NodeFootnoteReference { ix, .. } = &**node_footnote_reference;
                self.layout_footnote_reference(layout, node, *ix, range)
            }
            NodeValue::Highlight => self.layout_highlight(layout, node, range),
            NodeValue::HtmlInline(_) => self.layout_html_inline(layout, node, range),
            NodeValue::Image(_) => self.layout_image(layout, node, range),
            NodeValue::LineBreak => self.layout_line_break(layout, node, range),
            NodeValue::Link(_) => self.layout_link(layout, node, range),
            NodeValue::Math(_) => self.layout_math(layout, node, range),
            NodeValue::ShortCode(node_short_code) => {
                self.layout_short_code(layout, node, range, node_short_code)
            }
            NodeValue::SoftBreak => self.layout_soft_break(layout, node, range),
            NodeValue::SpoileredText => self.layout_spoilered_text(layout, node, range),
            NodeValue::Strikethrough => self.layout_strikethrough(layout, node, range),
            NodeValue::Strong => self.layout_strong(layout, node, range),
            NodeValue::Subscript => self.layout_subscript(layout, node, range),
            NodeValue::Superscript => self.layout_superscript(layout, node, range),
            NodeValue::Text(_) => self.layout_text(layout, node, range),
            NodeValue::Underline => self.layout_underline(layout, node, range),
            NodeValue::WikiLink(_) => self.layout_wikilink(layout, node, range),
        }
    }

    /// Walk an inline-container node's children, dispatching each.
    /// Counterpart to `show_inline_children`.
    pub fn layout_inline_children(
        &self,
        layout: &mut Layout,
        node: &'ast AstNode<'ast>,
        range: (Grapheme, Grapheme),
    ) {
        for child in node.children() {
            self.layout_inline(layout, child, range);
        }
    }

    /// Emit a circumfix inline (prefix / children / postfix bracketed
    /// by a `StyleOpen` / `StyleClose`). The visible-content `format`
    /// applies to the inner children + glued ranges; prefix/postfix
    /// syntax always uses `text_format_syntax()`. When syntax is
    /// hidden (cursor not on the node), prefix/postfix collapse to
    /// zero-visible `push_override` markers — these become row
    /// anchors so a cursor can still land "at the prefix" / "after
    /// the postfix" via hit-test.
    ///
    /// When the node has no children (comrak collapsed it — e.g. an
    /// empty `**` or stray `==`), the whole node range renders as
    /// syntax.
    pub fn layout_circumfix(
        &self,
        layout: &mut Layout,
        node: &'ast AstNode<'ast>,
        range: (Grapheme, Grapheme),
        format: Format,
    ) {
        let node_range = self.node_range(node);
        // Multi-line paragraphs dispatch every inline child to every
        // source line. Skip out-of-range nodes here, otherwise the
        // unconditional style_open/close below pollutes the line's
        // scope stack (and walker emits stray bg-pad fragments).
        if node_range.trim(&range).is_empty() {
            return;
        }
        let any_children = node.children().next().is_some();
        if !any_children {
            if range.contains_range(&node_range, true, true) {
                let trimmed = node_range.trim(&range);
                if !trimmed.is_empty() {
                    layout.push_source(trimmed, &self.buffer[trimmed], self.text_format_syntax());
                }
            }
            return;
        }
        layout.style_open(StyleInfo::new(format, node_range));
        let reveal = self.node_revealed(node);
        if let Some(prefix_range) = self.prefix_range(node) {
            let trimmed = prefix_range.trim(&range);
            if !trimmed.is_empty() {
                if reveal {
                    layout.push_source(trimmed, &self.buffer[trimmed], self.text_format_syntax());
                } else {
                    layout.push_override(
                        prefix_range.start().into_range(),
                        "",
                        self.text_format_syntax(),
                    );
                }
            }
        }
        if let Some(infix_range) = self.infix_range(node)
            && !infix_range.trim(&range).is_empty()
        {
            self.layout_inline_children(layout, node, range);
        }
        if let Some(postfix_range) = self.postfix_range(node) {
            let trimmed = postfix_range.trim(&range);
            if !trimmed.is_empty() {
                if reveal {
                    layout.push_source(trimmed, &self.buffer[trimmed], self.text_format_syntax());
                } else {
                    layout.push_override(
                        postfix_range.end().into_range(),
                        "",
                        self.text_format_syntax(),
                    );
                }
            }
        }
        layout.style_close();
    }
}
