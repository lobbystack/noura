use comrak::nodes::AstNode;
use editor_core::offset_types::{Grapheme, RangeExt as _, RangeIterExt as _};
use egui::{Pos2, Ui};

use crate::MdRender;
use crate::palette::ThemeExt as _;
use crate::widget::utils::wrap_layout::{FontFamily, Format, Layout};

impl<'ast> MdRender {
    pub fn text_format_document(&self) -> Format {
        Format {
            family: FontFamily::Sans,
            bold: false,
            italic: false,
            color: self.ctx.get_lb_theme().neutral_fg(),
            underline: false,
            strikethrough: false,
            background: egui::Color32::TRANSPARENT,
            border: egui::Color32::TRANSPARENT,
            spoiler: false,
            superscript: false,
            subscript: false,
        }
    }

    pub fn height_document(&self, node: &'ast AstNode<'ast>) -> f32 {
        let width = self.width(node);

        let any_children = node.children().next().is_some();
        if any_children && !self.plaintext {
            self.block_children_height(node)
        } else {
            let last = self.node_last_line_idx(node);
            let mut result = 0.;
            for line_idx in self.node_lines(node).iter() {
                result += self.height_source_line(line_idx, width);
                if line_idx != last {
                    result += self.layout.row_spacing;
                }
            }
            result
        }
    }

    pub fn show_document(&mut self, ui: &mut Ui, node: &'ast AstNode<'ast>, mut top_left: Pos2) {
        let width = self.width(node);

        let pre_lines = self.bounds.wrap_lines.len();
        let any_children = node.children().next().is_some();
        if any_children && !self.plaintext {
            self.show_block_children(ui, node, top_left);
        } else {
            let last = self.node_last_line_idx(node);
            for line_idx in self.node_lines(node).iter() {
                let h = self.show_source_line(ui, top_left, line_idx, width);
                top_left.y += h;
                if line_idx != last {
                    top_left.y += self.layout.row_spacing;
                }
            }
        }

        // Empty doc renders nothing — the cursor at offset 0 needs a
        // row anchor to land on. Compute a layout with a zero-visible
        // override at (0, 0) so `bounds.wrap_lines` gets an entry and
        // `fragment_at_pos`/`fragment_at_offset` can resolve.
        if self.bounds.wrap_lines.len() == pre_lines {
            let row_height = self.layout.row_height;
            let mut layout = Layout::new((Grapheme(0), Grapheme(0)));
            layout.push_override((Grapheme(0), Grapheme(0)), "", self.text_format_document());
            let result = self.compute_layout_from(layout, width, row_height);
            self.show_wrap_layout(ui, top_left, &result);
        }
    }

    /// Build a `Layout` for one source line of a plain-text file: one
    /// push_source for the whole line.
    fn layout_source_line(&self, line_idx: usize) -> Layout {
        let line = self.bounds.source_lines[line_idx];
        let mut layout = Layout::new(line);
        if self.mask {
            // One `*` per source byte keeps the `one_to_one` (byte-indexed)
            // mapping exact — cursor, selection, and iOS text rects line up
            // while the real text never reaches the screen.
            if !line.is_empty() {
                let masked = "*".repeat(self.buffer[line].len());
                layout.push_source(line, &masked, self.text_format_syntax());
            }
            return layout;
        }
        if !line.is_empty() {
            layout.push_source(line, &self.buffer[line], self.text_format_syntax());
        }
        layout
    }

    /// Renders one source line at `top_left` and returns its height.
    /// Used by both `show_document`'s plaintext branch and by
    /// `DocScrollContent` when iterating per-source-line rows.
    pub fn show_source_line(
        &mut self,
        ui: &mut Ui,
        top_left: Pos2,
        line_idx: usize,
        width: f32,
    ) -> f32 {
        let layout = self.layout_source_line(line_idx);
        let result = self.compute_layout_from(layout, width, self.layout.row_height);
        let h = result.height;
        self.show_wrap_layout(ui, top_left, &result);
        h
    }

    /// Cheap height estimate for a source line — no shaping. Plaintext
    /// rendering uses a monospace font, so chars-per-row × row_height
    /// approximates the wrapped height without going through cosmic-text.
    /// Suitable for `Rows::approx` in scroll virtualization.
    pub fn height_approx_source_line(&self, line_idx: usize, width: f32) -> f32 {
        let line = self.bounds.source_lines[line_idx];
        let chars = (line.end().0 - line.start().0) as f32;
        let row_height = self.layout.row_height;
        let char_width = row_height * 0.5;
        let chars_per_row = (width / char_width).floor().max(1.0);
        let rows = (chars / chars_per_row).ceil().max(1.0);
        rows * row_height + (rows - 1.0).max(0.0) * self.layout.row_spacing
    }

    /// Cheap measurement of a single source line.
    pub fn height_source_line(&self, line_idx: usize, width: f32) -> f32 {
        let layout = self.layout_source_line(line_idx);
        self.compute_layout_from(layout, width, self.layout.row_height)
            .height
    }
}
