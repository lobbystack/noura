use comrak::nodes::AstNode;
use editor_core::offset_types::Grapheme;

use crate::MdRender;
use crate::widget::utils::wrap_layout::Layout;

impl<'ast> MdRender {
    pub fn layout_soft_break(
        &self,
        layout: &mut Layout,
        node: &'ast AstNode<'ast>,
        range: (Grapheme, Grapheme),
    ) {
        self.layout_line_break(layout, node, range);
    }
}
