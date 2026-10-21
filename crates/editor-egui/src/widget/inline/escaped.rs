use comrak::nodes::AstNode;
use editor_core::offset_types::Grapheme;

use crate::MdRender;
use crate::widget::utils::wrap_layout::{Format, Layout};

impl<'ast> MdRender {
    pub fn text_format_escaped(&self, parent: &AstNode<'_>) -> Format {
        self.text_format(parent)
    }

    pub fn layout_escaped(
        &self,
        layout: &mut Layout,
        node: &'ast AstNode<'ast>,
        range: (Grapheme, Grapheme),
    ) {
        let fmt = self.text_format_escaped(node.parent().unwrap());
        self.layout_circumfix(layout, node, range, fmt);
    }
}
