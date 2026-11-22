use std::ops::Range;

use comrak::nodes::{AstNode, NodeValue};
use egui::{Pos2, Ui};

use crate::MdRender;

impl<'ast> MdRender {
    /// The space above and below each blank line before `node`. After a
    /// heading, the blank lines keep the text's own line rhythm instead of
    /// the space between blocks, so the heading sits nearer the content it
    /// introduces than the section above it. The lines stay in the file
    /// and stay editable; only the space drawn around them shrinks. A
    /// block right under its heading, with no blank line, keeps the usual
    /// space between blocks.
    fn pre_spacing_gap(&self, node: &'ast AstNode<'ast>, lines: &Range<usize>) -> f32 {
        let after_heading = node
            .previous_sibling()
            .is_some_and(|previous| matches!(previous.data.borrow().value, NodeValue::Heading(_)));
        if after_heading && !lines.is_empty() {
            self.layout.row_spacing / 2.0
        } else {
            self.layout.block_spacing
        }
    }

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
        let gap = self.pre_spacing_gap(node, &line_range);
        let mut result = 0.;
        if node.previous_sibling().is_some() {
            result += gap;
        }
        let n = line_range.end.saturating_sub(line_range.start) as f32;
        result += n * self.layout.row_height;
        result += n * gap;
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
        let gap = self.pre_spacing_gap(node, &line_range);
        let mut result = 0.;

        if node.previous_sibling().is_some() {
            result += gap;
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
            result += gap;
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
        let gap = self.pre_spacing_gap(node, &line_range);

        if node.previous_sibling().is_some() {
            top_left.y += gap;
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
            top_left.y += gap;
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
