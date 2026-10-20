use comrak::nodes::{AstNode, NodeHeading, NodeValue};
use egui::{Ui, UiBuilder};

use crate::palette::ThemeExt as _;
use crate::widget::utils::wrap_layout::{FontFamily, Format};

use super::MdRender;

pub(crate) mod block;
pub(crate) mod debug;
pub(crate) mod inline;
pub(crate) mod utils;

impl<'ast> MdRender {
    /// Creates a UI that assigns ids using the node range.
    // By default, egui ids are assigned to ui's and widgets based on the parent
    // ui's id and incremented with each addition to a given parent. Because
    // editor text may be clickable, text allocates ids and affects future ids.
    // When the editor reveal state changes, more or fewer interactable text
    // units may be shown, and all assigned ids may change. When an iOS user
    // taps the editor, iOS first sends a selection event in a standalone frame
    // which affects the reveal state, then by the time the tap is released, the
    // widget being tapped may have had its id changed and will not register as
    // clicked. This function creates a consistently idenified ui based on the
    // node range to prevent ids from changing mid tap and therefore prevents
    // taps from failing. Note that this range does not and need not survive
    // edits to the document itself.
    pub fn node_ui(&mut self, ui: &mut Ui, node: &'ast AstNode<'ast>) -> Ui {
        ui.new_child(
            UiBuilder::new()
                .id_salt(self.node_range(node)) // <- the magic
                .layer_id(ui.layer_id())
                .max_rect(ui.max_rect()),
        )
    }

    pub fn text_format(&self, node: &AstNode<'_>) -> Format {
        let parent = || node.parent().unwrap();
        let parent_text_format = || self.text_format(parent());

        match &node.data.borrow().value {
            NodeValue::FrontMatter(_) => parent_text_format(),
            NodeValue::Raw(_) => unreachable!("can only be created programmatically"),

            // container_block
            NodeValue::Alert(node_alert) => self.text_format_alert(parent(), node_alert),
            NodeValue::BlockQuote => self.text_format_block_quote(parent()),
            NodeValue::DescriptionItem(_) => unimplemented!("extension disabled"),
            NodeValue::DescriptionList => unimplemented!("extension disabled"),
            NodeValue::Document => self.text_format_document(),
            NodeValue::FootnoteDefinition(_) => self.text_format_footnote_definition(parent()),
            NodeValue::Item(_) => parent_text_format(),
            NodeValue::List(_) => parent_text_format(),
            NodeValue::MultilineBlockQuote(_) => unimplemented!("extension disabled"),
            NodeValue::Table(_) => parent_text_format(),
            NodeValue::TableRow(is_header_row) => {
                self.text_format_table_row(parent(), *is_header_row)
            }

            // inline
            NodeValue::Code(_) => self.text_format_code(parent()),
            NodeValue::Emph => self.text_format_emph(parent()),
            NodeValue::Escaped => self.text_format_escaped(parent()),
            NodeValue::EscapedTag(_) => self.text_format_escaped_tag(parent()),
            NodeValue::FootnoteReference(_) => self.text_format_footnote_reference(parent()),
            NodeValue::Highlight => self.text_format_highlight(parent()),
            NodeValue::HtmlInline(_) => self.text_format_html_inline(parent()),
            NodeValue::Image(ni) => {
                self.text_format_link(parent(), self.link_state_for_url(&ni.url))
            }
            NodeValue::LineBreak => parent_text_format(),
            NodeValue::Link(nl) => {
                self.text_format_link(parent(), self.link_state_for_url(&nl.url))
            }
            NodeValue::Math(_) => self.text_format_math(parent()),
            NodeValue::ShortCode(_) => self.text_format_short_code(parent()),
            NodeValue::SoftBreak => parent_text_format(),
            NodeValue::SpoileredText => self.text_format_spoilered_text(node, parent()),
            NodeValue::Strikethrough => self.text_format_strikethrough(parent()),
            NodeValue::Strong => self.text_format_strong(parent()),
            NodeValue::Subscript => self.text_format_subscript(parent()),
            NodeValue::Subtext => unimplemented!("extension disabled"),
            NodeValue::Superscript => self.text_format_superscript(parent()),
            NodeValue::Text(_) => parent_text_format(),
            NodeValue::Underline => self.text_format_underline(parent()),
            NodeValue::WikiLink(nwl) => {
                self.text_format_link(parent(), self.link_state_for_wikilink(&nwl.url))
            }

            // leaf_block
            NodeValue::CodeBlock(_) => self.text_format_code_block(parent()),
            NodeValue::DescriptionDetails => unimplemented!("extension disabled"),
            NodeValue::DescriptionTerm => unimplemented!("extension disabled"),
            NodeValue::Heading(_) => parent_text_format(),
            NodeValue::HtmlBlock(_) => self.text_format_html_block(parent()),
            NodeValue::Paragraph => parent_text_format(),
            NodeValue::TableCell => parent_text_format(),
            NodeValue::TaskItem(_) => parent_text_format(),
            NodeValue::ThematicBreak => parent_text_format(),
        }
    }

    pub fn text_format_syntax(&self) -> Format {
        Format {
            family: FontFamily::Mono,
            bold: false,
            italic: false,
            color: self.ctx.get_lb_theme().neutral_fg_secondary(),
            underline: false,
            strikethrough: false,
            background: egui::Color32::TRANSPARENT,
            border: egui::Color32::TRANSPARENT,
            spoiler: false,
            superscript: false,
            subscript: false,
        }
    }

    pub fn row_height(&self, node: &AstNode<'_>) -> f32 {
        match &node.data.borrow().value {
            NodeValue::Heading(NodeHeading { level, .. }) => self.heading_row_height(*level),
            _ => self.layout.row_height,
        }
    }
}
