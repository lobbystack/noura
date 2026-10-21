use comrak::nodes::AstNode;
use egui::{Pos2, Ui};

use crate::MdRender;

impl<'ast> MdRender {
    /// Cheap pre-spacing height estimate. Each blank line is one
    /// `row_height`; plus inter-block `block_spacing`. Used by
    /// `height_approx` for off-screen content.
    pub fn block_pre_spacing_height_approx(&self, node: &'ast AstNode<'ast>) -> f32 {
        if self.hidden_by_fold(node) {
            return 0.;
        }
        let Some(line_range) = self.pre_spacing_lines(node) else {
            return 0.;
        };
        let mut result = 0.;
        if node.previous_sibling().is_some() {
            result += self.layout.block_spacing;
        }
        let n = line_range.end.saturating_sub(line_range.start) as f32;
        result += n * self.layout.row_height;
        result += n * self.layout.block_spacing;
        result
    }

    /// Cheap post-spacing height estimate.
    pub fn block_post_spacing_height_approx(&self, node: &'ast AstNode<'ast>) -> f32 {
        let Some(line_range) = self.post_spacing_lines(node) else {
            return 0.;
        };
        let n = line_range.end.saturating_sub(line_range.start) as f32;
        n * self.layout.row_height + n * self.layout.block_spacing
    }

    pub fn block_pre_spacing_height(&self, node: &'ast AstNode<'ast>) -> f32 {
        let Some(line_range) = self.pre_spacing_lines(node) else {
            return 0.;
        };

        let width = self.width(node);
        let row_height = self.layout.row_height;
        let mut result = 0.;

        if node.previous_sibling().is_some() {
            result += self.layout.block_spacing;
        }

        for line_idx in line_range {
            let line = self.bounds.source_lines[line_idx];
            let node_line = self.node_line(node, line);
            let l = self.compute_section_layout_new(
                node_line,
                width,
                row_height,
                self.text_format_document(),
            );
            result += l.height;
            result += self.layout.block_spacing;
        }

        result
    }

    pub fn show_block_pre_spacing(
        &mut self,
        ui: &mut Ui,
        node: &'ast AstNode<'ast>,
        mut top_left: Pos2,
    ) {
        let Some(line_range) = self.pre_spacing_lines(node) else {
            return;
        };

        let width = self.width(node);
        let row_height = self.layout.row_height;

        if node.previous_sibling().is_some() {
            top_left.y += self.layout.block_spacing;
        }

        for line_idx in line_range {
            let line = self.bounds.source_lines[line_idx];
            let node_line = self.node_line(node, line);
            let result = self.compute_section_layout_new(
                node_line,
                width,
                row_height,
                self.text_format_document(),
            );
            let h = result.height;
            self.show_wrap_layout(ui, top_left, &result);
            self.show_block_line_prefixes(ui, node, line, top_left, row_height);
            top_left.y += h;
            top_left.y += self.layout.block_spacing;
        }
    }

    pub(crate) fn block_post_spacing_height(&self, node: &'ast AstNode<'ast>) -> f32 {
        let Some(line_range) = self.post_spacing_lines(node) else {
            return 0.;
        };

        let width = self.width(node);
        let row_height = self.layout.row_height;
        let mut result = 0.;

        for line_idx in line_range {
            let line = self.bounds.source_lines[line_idx];
            let node_line = self.node_line(node, line);

            result += self.layout.block_spacing;
            let l = self.compute_section_layout_new(
                node_line,
                width,
                row_height,
                self.text_format_document(),
            );
            result += l.height;
        }

        result
    }

    pub(crate) fn show_block_post_spacing(
        &mut self,
        ui: &mut Ui,
        node: &'ast AstNode<'ast>,
        mut top_left: Pos2,
    ) {
        let Some(line_range) = self.post_spacing_lines(node) else {
            return;
        };

        let width = self.width(node);
        let row_height = self.layout.row_height;

        for line_idx in line_range {
            let line = self.bounds.source_lines[line_idx];
            let node_line = self.node_line(node, line);

            top_left.y += self.layout.block_spacing;
            let result = self.compute_section_layout_new(
                node_line,
                width,
                row_height,
                self.text_format_document(),
            );
            let h = result.height;
            self.show_wrap_layout(ui, top_left, &result);
            self.show_block_line_prefixes(ui, node, line, top_left, row_height);
            top_left.y += h;
        }
    }
}
