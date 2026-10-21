use comrak::nodes::AstNode;
use editor_core::offset_types::{Grapheme, RangeExt as _};

use crate::MdRender;
use crate::palette::ThemeExt as _;
use crate::widget::utils::wrap_layout::{Format, Layout};

impl<'ast> MdRender {
    pub fn text_format_footnote_reference(&self, parent: &AstNode<'_>) -> Format {
        Format {
            color: self.ctx.get_lb_theme().neutral_fg_secondary(),
            ..self.text_format_superscript(parent)
        }
    }

    pub fn layout_footnote_reference(
        &self,
        layout: &mut Layout,
        node: &'ast AstNode<'ast>,
        ix: u32,
        range: (Grapheme, Grapheme),
    ) {
        let node_range = self.node_range(node);
        if !range.contains_range(&node_range, true, true) {
            return;
        }
        if self.node_revealed(node) {
            let prefix = (node_range.start(), node_range.start() + 2);
            let infix = (node_range.start() + 2, node_range.end() - 1);
            let postfix = (node_range.end() - 1, node_range.end());
            layout.push_source(prefix, &self.buffer[prefix], self.text_format_syntax());
            layout.push_source(
                infix,
                &self.buffer[infix],
                self.text_format_footnote_reference(node.parent().unwrap()),
            );
            layout.push_source(postfix, &self.buffer[postfix], self.text_format_syntax());
        } else {
            let text = format!("{ix}");
            layout.push_override(
                node_range,
                &text,
                self.text_format_footnote_reference(node.parent().unwrap()),
            );
        }
    }
}
