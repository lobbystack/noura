use comrak::nodes::AstNode;
use editor_core::offset_types::Grapheme;

use crate::MdRender;
use crate::widget::utils::wrap_layout::{Format, Layout};

impl<'ast> MdRender {
    pub fn text_format_strikethrough(&self, parent: &AstNode<'_>) -> Format {
        let parent_text_format = self.text_format(parent);
        Format {
            strikethrough: true,
            ..parent_text_format
        }
    }

    pub fn layout_strikethrough(
        &self,
        layout: &mut Layout,
        node: &'ast AstNode<'ast>,
        range: (Grapheme, Grapheme),
    ) {
        let fmt = self.text_format_strikethrough(node.parent().unwrap());
        self.layout_circumfix(layout, node, range, fmt);
    }
}
