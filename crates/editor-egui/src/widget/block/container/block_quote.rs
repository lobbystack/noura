use comrak::nodes::AstNode;
use editor_core::offset_types::RangeIterExt as _;
use egui::{Pos2, Rect, Stroke, Ui, Vec2};

use crate::MdRender;
use crate::widget::utils::wrap_layout::Format;

use crate::palette::ThemeExt as _;

impl<'ast> MdRender {
    pub fn text_format_block_quote(&self, parent: &AstNode<'_>) -> Format {
        let parent_text_format = self.text_format(parent);
        Format {
            color: self.ctx.get_lb_theme().neutral_fg_secondary(),
            ..parent_text_format
        }
    }

    pub fn height_block_quote(&self, node: &'ast AstNode<'ast>) -> f32 {
        let mut result = 0.;
        let width = self.width(node);
        let row_height = self.layout.row_height;

        let first_line_idx = self.node_first_line_idx(node);
        let any_children = node.children().next().is_some();
        if any_children {
            result += self.block_children_height(node)
        } else {
            for line_idx in self.node_lines(node).iter() {
                let line = self.bounds.source_lines[line_idx];
                let line_content = self.line_content(node, line);

                if line_idx != first_line_idx {
                    result += self.layout.block_spacing;
                }
                let l = self.compute_section_layout_new(
                    line_content,
                    width,
                    row_height,
                    self.text_format_syntax(),
                );
                result += l.height;
            }
        }

        result
    }

    pub fn show_block_quote(&mut self, ui: &mut Ui, node: &'ast AstNode<'ast>, mut top_left: Pos2) {
        let height = self.height(node);
        let annotation_size = Vec2 {
            x: self.layout.indent,
            y: height,
        };
        let annotation_space = Rect::from_min_size(top_left, annotation_size);

        // when revealed, the raw `>` prefix occupies this column instead
        if !self.reveal(node) {
            ui.painter().vline(
                annotation_space.center().x,
                annotation_space.y_range(),
                Stroke::new(3_f32, self.ctx.get_lb_theme().neutral_bg_tertiary()),
            );
        }

        top_left.x += annotation_space.width();
        let width = self.width(node);
        let row_height = self.layout.row_height;

        let first_line_idx = self.node_first_line_idx(node);
        let any_children = node.children().next().is_some();
        if any_children {
            self.show_block_children(ui, node, top_left);
        } else {
            for line_idx in self.node_lines(node).iter() {
                let line = self.bounds.source_lines[line_idx];
                let line_content = self.line_content(node, line);

                if line_idx != first_line_idx {
                    top_left.y += self.layout.block_spacing;
                }

                let result = self.compute_section_layout_new(
                    line_content,
                    width,
                    row_height,
                    self.text_format_syntax(),
                );
                let h = result.height;
                self.show_wrap_layout(ui, top_left, &result);
                self.show_block_line_prefixes(ui, node, line, top_left, row_height);
                top_left.y += h;
            }
        }
    }
}
