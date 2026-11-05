use comrak::nodes::AstNode;
use editor_core::offset_types::{Grapheme, RangeExt as _};

use crate::MdRender;
use crate::palette::ThemeExt as _;
use crate::widget::utils::wrap_layout::{Format, Layout};

impl<'ast> MdRender {
    pub fn text_format_html_inline(&self, parent: &AstNode<'_>) -> Format {
        Format {
            color: self.ctx.get_lb_theme().neutral_fg_secondary(),
            background: self.ctx.get_lb_theme().neutral_bg_secondary(),
            ..self.text_format_code_block(parent)
        }
    }

    pub fn layout_html_inline(
        &self,
        layout: &mut Layout,
        node: &'ast AstNode<'ast>,
        range: (Grapheme, Grapheme),
    ) {
        let node_range = self.node_range(node).trim(&range);
        if node_range.is_empty() {
            return;
        }
        // A tag that's actively folding a section renders as a `···`
        // chip, never as source.
        if let Some(fold) = self.active_fold_at_tag(self.node_range(node)) {
            self.layout_fold_chip(layout, node.parent().unwrap(), fold, node_range);
            return;
        }
        // HTML the editor doesn't draw (`<span>`, `<!-- -->` and the like)
        // stays visible as the text you wrote, as today's editor shows it.
        layout.push_source(
            node_range,
            &self.buffer[node_range],
            self.text_format_syntax(),
        );
    }
}
