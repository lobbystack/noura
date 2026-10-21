use crate::palette::ThemeExt as _;
use crate::widget::utils::wrap_layout::Format;
use comrak::nodes::{AlertType, AstNode, NodeAlert};
use editor_core::offset_types::RangeIterExt as _;
use egui::{Pos2, Rect, Stroke, Ui, Vec2};

use crate::MdRender;

impl<'ast> MdRender {
    pub fn text_format_alert(&self, parent: &AstNode<'_>, node_alert: &NodeAlert) -> Format {
        let parent_text_format = self.text_format(parent);
        Format {
            color: match node_alert.alert_type {
                AlertType::Note => self.ctx.get_lb_theme().fg().blue,
                AlertType::Tip => self.ctx.get_lb_theme().fg().green,
                AlertType::Important => self.ctx.get_lb_theme().fg().magenta,
                AlertType::Warning => self.ctx.get_lb_theme().fg().yellow,
                AlertType::Caution => self.ctx.get_lb_theme().fg().red,
            },
            ..parent_text_format
        }
    }

    pub fn height_alert(&self, node: &'ast AstNode<'ast>, node_alert: &NodeAlert) -> f32 {
        let mut result = self.height_alert_title_line(node, node_alert);
        let width = self.width(node);
        let row_height = self.layout.row_height;

        let first_line_idx = self.node_first_line_idx(node);
        let any_children = node.children().next().is_some();
        if any_children {
            result += self.layout.block_spacing;
            result += self.block_children_height(node)
        } else {
            for line_idx in self.node_lines(node).iter() {
                let line = self.bounds.source_lines[line_idx];
                let line_content = self.line_content(node, line);

                if line_idx != first_line_idx {
                    result += self.layout.block_spacing;
                    let l = self.compute_section_layout_new(
                        line_content,
                        width,
                        row_height,
                        self.text_format_syntax(),
                    );
                    result += l.height;
                }
            }
        }

        result
    }

    pub fn show_alert(
        &mut self,
        ui: &mut Ui,
        node: &'ast AstNode<'ast>,
        mut top_left: Pos2,
        node_alert: &NodeAlert,
    ) {
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
                Stroke::new(3_f32, self.text_format(node).color),
            );
        }

        top_left.x += self.layout.indent;
        let width = self.width(node) - self.layout.indent;
        let row_height = self.layout.row_height;

        // title line is shown & revealed separately from block syntax as if
        // it's a child block - see also: special handling in spacing.rs
        let first_line = self.node_first_line(node);
        self.show_alert_title_line(ui, node, top_left, node_alert);
        self.show_block_line_prefixes(ui, node, first_line, top_left, row_height);
        top_left.y += self.height_alert_title_line(node, node_alert);

        let any_children = node.children().next().is_some();
        if any_children {
            top_left.y += self.layout.block_spacing;
            self.show_block_children(ui, node, top_left);
        } else {
            for line in self.node_lines(node).iter() {
                let line = self.bounds.source_lines[line];
                let line_content = self.line_content(node, line);

                if line != first_line {
                    top_left.y += self.layout.block_spacing;
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

    pub fn height_alert_title_line(
        &self,
        node: &'ast AstNode<'ast>,
        node_alert: &NodeAlert,
    ) -> f32 {
        let row_height = self.layout.row_height;
        let line = self.node_first_line(node);
        let line_content = self.line_content(node, line);
        if self.range_revealed(line_content, true) {
            let width = self.width(node) - self.layout.indent;
            self.compute_section_layout_new(
                line_content,
                width,
                row_height,
                self.text_format_syntax(),
            )
            .height
        } else {
            let title_width =
                self.width(node) - self.layout.indent - self.layout.row_height - self.layout.indent;
            let (_type, title) = self.alert_type_title_ranges(node);
            if node_alert.title.is_some() {
                self.compute_section_layout_new(
                    title,
                    title_width,
                    row_height,
                    self.text_format(node),
                )
                .height
            } else {
                let type_display_text = match node_alert.alert_type {
                    AlertType::Note => "Note",
                    AlertType::Tip => "Tip",
                    AlertType::Important => "Important",
                    AlertType::Warning => "Warning",
                    AlertType::Caution => "Caution",
                };
                self.compute_override_section_layout_new(
                    title,
                    type_display_text,
                    title_width,
                    row_height,
                    self.text_format(node),
                )
                .height
            }
        }
    }

    pub fn show_alert_title_line(
        &mut self,
        ui: &mut Ui,
        node: &'ast AstNode<'ast>,
        top_left: Pos2,
        node_alert: &NodeAlert,
    ) {
        let row_height = self.layout.row_height;
        let line = self.node_first_line(node);
        let line_content = self.line_content(node, line);
        if self.range_revealed(line_content, true) {
            // note and title line are revealed separately from block syntax as
            // if they're a child block
            let width = self.width(node) - self.layout.indent;
            let result = self.compute_section_layout_new(
                line_content,
                width,
                row_height,
                self.text_format_syntax(),
            );
            self.show_wrap_layout(ui, top_left, &result);
        } else {
            let icon_space = Rect::from_min_size(top_left, Vec2::splat(self.layout.row_height));
            let display_text_top_left = top_left + Vec2::X * self.layout.indent;
            let title_width =
                self.width(node) - self.layout.indent - self.layout.row_height - self.layout.indent;

            // icon: a dot in the alert's color
            ui.painter().circle_filled(
                icon_space.center(),
                icon_space.height() * 0.25,
                self.text_format(node).color,
            );

            let (_type, title) = self.alert_type_title_ranges(node);
            if node_alert.title.is_some() {
                let result = self.compute_section_layout_new(
                    title,
                    title_width,
                    row_height,
                    self.text_format(node),
                );
                self.show_wrap_layout(ui, display_text_top_left, &result);
            } else {
                let type_display_text = match node_alert.alert_type {
                    AlertType::Note => "Note",
                    AlertType::Tip => "Tip",
                    AlertType::Important => "Important",
                    AlertType::Warning => "Warning",
                    AlertType::Caution => "Caution",
                };
                let result = self.compute_override_section_layout_new(
                    title,
                    type_display_text,
                    title_width,
                    row_height,
                    self.text_format(node),
                );
                self.show_wrap_layout(ui, display_text_top_left, &result);
            }
        }
    }
}
