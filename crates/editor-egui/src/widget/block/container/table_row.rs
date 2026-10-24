use comrak::nodes::AstNode;
use egui::accesskit::Role;
use egui::{Pos2, Rangef, Rect, Sense, Stroke, Ui, Vec2};

use crate::MdRender;
use crate::palette::ThemeExt as _;
use crate::widget::utils::wrap_layout::Format;

impl<'ast> MdRender {
    /// Shows a grid cell to screen readers as a cell labeled with its text.
    /// Clicks pass through to the editor, which puts the caret in the
    /// cell's source.
    fn expose_table_cell(
        &self,
        ui: &mut Ui,
        cell: &'ast AstNode<'ast>,
        rect: Rect,
        is_header_row: bool,
    ) {
        let range = self.node_range(cell);
        let label = self.buffer[range].trim().to_string();
        let response = ui.interact(
            rect,
            ui.scope_id().with(("md_table_cell", range)),
            Sense::hover(),
        );
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_role(if is_header_row {
                Role::ColumnHeader
            } else {
                Role::Cell
            });
            node.set_label(label);
            node.set_bounds(egui::accesskit::Rect {
                x0: rect.min.x.into(),
                y0: rect.min.y.into(),
                x1: rect.max.x.into(),
                y1: rect.max.y.into(),
            });
        });
    }

    pub fn text_format_table_row(&self, parent: &AstNode<'_>, is_header_row: bool) -> Format {
        let parent_text_format = self.text_format(parent);
        Format {
            bold: is_header_row,
            ..parent_text_format
        }
    }

    pub fn height_table_row(&self, node: &'ast AstNode<'ast>) -> f32 {
        if self.reveal_table_row(node) {
            let line = self.node_first_line(node);
            let node_line = self.node_line(node, line);
            let width = self.width(node);
            self.layout.block_padding
                + self
                    .compute_section_layout_new(
                        node_line,
                        width,
                        self.layout.row_height,
                        self.text_format_syntax(),
                    )
                    .height
                + self.layout.block_padding
        } else {
            // the height of the row is the height of the tallest cell
            let mut cell_height_max = 0.0f32;
            for table_cell in node.children() {
                cell_height_max = cell_height_max.max(self.height(table_cell));
            }

            cell_height_max
        }
    }

    pub fn show_table_row(
        &mut self,
        ui: &mut Ui,
        node: &'ast AstNode<'ast>,
        mut top_left: Pos2,
        is_header_row: bool,
    ) {
        if self.reveal_table_row(node) {
            top_left += Vec2::splat(self.layout.block_padding);

            let line = self.node_first_line(node);
            let node_line = self.node_line(node, line);
            let width = self.width(node);
            let result = self.compute_section_layout_new(
                node_line,
                width,
                self.layout.row_height,
                self.text_format_syntax(),
            );
            self.show_wrap_layout(ui, top_left, &result);
        } else {
            let height = self.height_table_row(node);
            let width = self.width(node);
            let child_width = width / node.children().count() as f32;

            // draw row backgrounds
            let row_rect = Rect::from_min_size(top_left, Vec2::new(width, height));
            if is_header_row {
                ui.painter().rect_filled(
                    row_rect,
                    1.,
                    self.ctx.get_lb_theme().neutral_bg_secondary(),
                );
            }

            // draw interior decorations
            let stroke = Stroke {
                width: 1.,
                color: self.ctx.get_lb_theme().neutral_bg_tertiary(),
            };
            if !is_header_row {
                ui.painter().hline(
                    Rangef::new(top_left.x, top_left.x + width),
                    top_left.y,
                    stroke,
                );
            }
            for child_idx in 1..node.children().count() {
                ui.painter().vline(
                    top_left.x + child_idx as f32 * child_width,
                    Rangef::new(top_left.y, top_left.y + height),
                    stroke,
                );
            }

            // draw cell contents
            let mut child_top_left = top_left;
            for table_cell in node.children() {
                self.show_table_cell(ui, table_cell, child_top_left);
                let cell_rect = Rect::from_min_size(child_top_left, Vec2::new(child_width, height));
                self.expose_table_cell(ui, table_cell, cell_rect, is_header_row);
                child_top_left.x += child_width;
            }
        }
    }
}
