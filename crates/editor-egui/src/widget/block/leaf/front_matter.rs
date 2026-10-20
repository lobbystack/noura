use comrak::nodes::AstNode;
use egui::{Pos2, Ui};

use crate::MdRender;

impl<'ast> MdRender {
    pub fn height_front_matter(&self, node: &'ast AstNode<'ast>) -> f32 {
        self.height_fenced_code_block(node, &editor_core::MdDoc::frontmatter_as_code_block())
    }

    pub fn show_front_matter(&mut self, ui: &mut Ui, node: &'ast AstNode<'ast>, top_left: Pos2) {
        self.show_fenced_code_block(
            ui,
            node,
            top_left,
            &editor_core::MdDoc::frontmatter_as_code_block(),
        );
    }
}
