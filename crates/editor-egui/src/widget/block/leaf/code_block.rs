use comrak::nodes::{AstNode, NodeCodeBlock};
use editor_core::highlight::CodeStyle;
use editor_core::offset_types::{Grapheme, RangeExt as _};
use egui::{Pos2, Rect, Stroke, Ui, Vec2};

use crate::MdRender;
use crate::widget::utils::wrap_layout::{FontFamily, Format, Layout};

use crate::palette::{ThemeExt as _, translucent_over};

impl<'ast> MdRender {
    pub fn text_format_code_block(&self, parent: &AstNode<'_>) -> Format {
        let parent_text_format = self.text_format(parent);
        Format {
            family: FontFamily::Mono,
            ..parent_text_format
        }
    }

    pub fn height_code_block(
        &self,
        node: &'ast AstNode<'ast>,
        node_code_block: &NodeCodeBlock,
    ) -> f32 {
        if node_code_block.fenced {
            self.height_fenced_code_block(node, node_code_block)
        } else {
            self.height_indented_code_block(node, node_code_block, false)
        }
    }

    pub fn show_code_block(
        &mut self,
        ui: &mut Ui,
        node: &'ast AstNode<'ast>,
        top_left: Pos2,
        node_code_block: &NodeCodeBlock,
    ) {
        if node_code_block.fenced {
            self.show_fenced_code_block(ui, node, top_left, node_code_block);
        } else {
            self.show_indented_code_block(ui, node, top_left, node_code_block, false);
        }
    }

    pub fn height_fenced_code_block(
        &self,
        node: &'ast AstNode<'ast>,
        node_code_block: &NodeCodeBlock,
    ) -> f32 {
        let width = self.width(node) - 2. * self.layout.block_padding;
        let row_height = self.layout.row_height;

        let mut result = self.layout.block_padding;
        result -= self.layout.row_spacing;

        let reveal = self.reveal_fenced_code_block(node, node_code_block);
        let first_line_idx = self.node_first_line_idx(node);
        let last_line_idx = self.node_last_line_idx(node);
        for line_idx in first_line_idx..=last_line_idx {
            let line = self.bounds.source_lines[line_idx];
            let node_line = self.node_line(node, line);

            let is_opening_fence = line_idx == first_line_idx;
            let is_closing_fence = !is_opening_fence
                && line_idx == last_line_idx
                && self.is_closing_fence(node, node_code_block, line);

            if is_opening_fence || is_closing_fence {
                if reveal {
                    result += self.layout.row_spacing;
                    let fence_layout = self.compute_section_layout_new(
                        node_line,
                        width,
                        row_height,
                        self.text_format_syntax(),
                    );
                    result += fence_layout.height;
                }
            } else {
                result += self.layout.row_spacing;
                result += self.height_code_block_line(node, node_code_block, line, false);
            }
        }

        result + self.layout.block_padding
    }

    pub fn show_fenced_code_block(
        &mut self,
        ui: &mut Ui,
        node: &'ast AstNode<'ast>,
        mut top_left: Pos2,
        node_code_block: &NodeCodeBlock,
    ) {
        let mut width = self.width(node);
        let height = self.height_fenced_code_block(node, node_code_block);
        let row_height = self.layout.row_height;

        let rect = Rect::from_min_size(top_left, Vec2::new(width, height));
        let theme = self.ctx.get_lb_theme();
        if let Some(code) = theme.code_variant() {
            let fill = translucent_over(code.background, theme.neutral_bg(), 0.5);
            ui.painter().rect_filled(rect, 2., fill);
        }
        ui.painter().rect_stroke(
            rect,
            2.,
            Stroke::new(1_f32, theme.neutral_bg_tertiary()),
            egui::epaint::StrokeKind::Inside,
        );

        width -= 2. * self.layout.block_padding;
        // Container gutter columns sit at the code block's outer left (the
        // deepest container's content origin), not inside the padded box.
        let content_left = top_left.x;
        top_left.x += self.layout.block_padding;
        top_left.y += self.layout.block_padding;
        top_left.y -= self.layout.row_spacing; // makes spacing logic simpler

        let reveal = self.reveal_fenced_code_block(node, node_code_block);
        let first_line_idx = self.node_first_line_idx(node);
        let last_line_idx = self.node_last_line_idx(node);
        for line_idx in first_line_idx..=last_line_idx {
            let line = self.bounds.source_lines[line_idx];
            let node_line = self.node_line(node, line);

            let is_opening_fence = line_idx == first_line_idx;
            let is_closing_fence = !is_opening_fence
                && line_idx == last_line_idx
                && self.is_closing_fence(node, node_code_block, line);

            if is_opening_fence || is_closing_fence {
                if reveal {
                    top_left.y += self.layout.row_spacing;
                    let fence_layout = self.compute_section_layout_new(
                        node_line,
                        width,
                        row_height,
                        self.text_format_syntax(),
                    );
                    let h = fence_layout.height;
                    self.show_wrap_layout(ui, top_left, &fence_layout);
                    self.show_block_line_prefixes(
                        ui,
                        node,
                        line,
                        Pos2::new(content_left, top_left.y),
                        row_height,
                    );
                    top_left.y += h;
                }
            } else {
                top_left.y += self.layout.row_spacing;
                self.show_code_block_line(ui, node, top_left, node_code_block, line, false);
                self.show_block_line_prefixes(
                    ui,
                    node,
                    line,
                    Pos2::new(content_left, top_left.y),
                    row_height,
                );
                top_left.y += self.height_code_block_line(node, node_code_block, line, false);
            }
        }
    }

    pub fn height_indented_code_block(
        &self,
        node: &'ast AstNode<'ast>,
        node_code_block: &NodeCodeBlock,
        synthetic: bool,
    ) -> f32 {
        let mut result = 0.;
        let width = self.width(node) - 2. * self.layout.block_padding;
        let row_height = self.layout.row_height;

        let reveal = self.reveal_indented_code_block(node, synthetic);
        let first_line_idx = self.node_first_line_idx(node);
        let last_line_idx = self.node_last_line_idx(node);
        for line_idx in first_line_idx..=last_line_idx {
            let line = self.bounds.source_lines[line_idx];

            if reveal {
                let node_line = self.node_line(node, line);
                let l = self.compute_section_layout_new(
                    node_line,
                    width,
                    row_height,
                    self.text_format_syntax(),
                );
                result += l.height;
            } else {
                result += self.height_code_block_line(node, node_code_block, line, synthetic);
            }

            if line_idx != last_line_idx {
                result += self.layout.row_spacing;
            }
        }

        result + 2. * self.layout.block_padding
    }

    // indented code blocks don't have an info string; the `info` parameter is
    // just used for syntax highlighting when rendering unsupported node types
    // like html blocks as code
    pub fn show_indented_code_block(
        &mut self,
        ui: &mut Ui,
        node: &'ast AstNode<'ast>,
        mut top_left: Pos2,
        node_code_block: &NodeCodeBlock,
        synthetic: bool,
    ) {
        let width = self.width(node);
        let inner_width = width - 2. * self.layout.block_padding;
        let row_height = self.layout.row_height;
        let height = self.height_indented_code_block(node, node_code_block, synthetic);

        let rect = Rect::from_min_size(top_left, Vec2::new(width, height));
        let theme = self.ctx.get_lb_theme();
        if let Some(code) = theme.code_variant() {
            let fill = translucent_over(code.background, theme.neutral_bg(), 0.5);
            ui.painter().rect_filled(rect, 2., fill);
        }
        ui.painter().rect_stroke(
            rect,
            2.,
            Stroke::new(1_f32, theme.neutral_bg_tertiary()),
            egui::epaint::StrokeKind::Inside,
        );

        // Container gutter columns sit at the code block's outer left (the
        // deepest container's content origin), not inside the padded box.
        let content_left = top_left.x;
        top_left.x += self.layout.block_padding;
        top_left.y += self.layout.block_padding;

        let reveal = self.reveal_indented_code_block(node, synthetic);
        let first_line_idx = self.node_first_line_idx(node);
        let last_line_idx = self.node_last_line_idx(node);
        for line_idx in first_line_idx..=last_line_idx {
            let line = self.bounds.source_lines[line_idx];

            if reveal {
                let node_line = self.node_line(node, line);
                let l = self.compute_section_layout_new(
                    node_line,
                    inner_width,
                    row_height,
                    self.text_format_syntax(),
                );
                let h = l.height;
                self.show_wrap_layout(ui, top_left, &l);
                self.show_block_line_prefixes(
                    ui,
                    node,
                    line,
                    Pos2::new(content_left, top_left.y),
                    row_height,
                );
                top_left.y += h;
            } else {
                self.show_code_block_line(ui, node, top_left, node_code_block, line, synthetic);
                self.show_block_line_prefixes(
                    ui,
                    node,
                    line,
                    Pos2::new(content_left, top_left.y),
                    row_height,
                );
                top_left.y += self.height_code_block_line(node, node_code_block, line, synthetic);
            }

            top_left.y += self.layout.row_spacing;
        }
    }

    /// Build a `Layout` for one code-block line in the code format, with
    /// the pieces the block's language highlights in their colors when
    /// `colors` asks. Colors leave sizes alone, so heights skip them.
    fn layout_code_block_line(
        &self,
        node: &'ast AstNode<'ast>,
        node_code_block: &NodeCodeBlock,
        line: (Grapheme, Grapheme),
        synthetic: bool,
        colors: bool,
    ) -> Layout {
        let node_line = self.node_line(node, line);
        let code_line = self.code_line_range(node, node_code_block, node_line, synthetic);
        let mut layout = Layout::new(code_line);
        if code_line.is_empty() {
            return layout;
        }
        let format = self.text_format(node);
        let highlights = if synthetic || !colors {
            Vec::new()
        } else {
            self.code_block_highlights(node, node_code_block)
        };
        let mut from = code_line.start();
        for (range, style) in highlights
            .into_iter()
            .filter(|(range, _)| range.intersects(&code_line, false))
        {
            let range = range.trim(&code_line);
            if from < range.start() {
                let plain = (from, range.start());
                layout.push_source(plain, &self.buffer[plain], format.clone());
            }
            let styled = Format {
                color: self.code_color(style),
                ..format.clone()
            };
            layout.push_source(range, &self.buffer[range], styled);
            from = range.end();
        }
        if from < code_line.end() {
            let plain = (from, code_line.end());
            layout.push_source(plain, &self.buffer[plain], format);
        }
        layout
    }

    /// The color highlighted code of `style` draws in.
    fn code_color(&self, style: CodeStyle) -> egui::Color32 {
        let theme = self.ctx.get_lb_theme();
        if let Some(code) = theme.code_variant() {
            return match style {
                CodeStyle::Keyword => code.keyword,
                CodeStyle::Function => code.function,
                CodeStyle::Type => code.class,
                CodeStyle::String => code.string,
                CodeStyle::Number => code.number,
                CodeStyle::Comment => code.comment,
            };
        }
        let fg = theme.fg();
        match style {
            CodeStyle::Keyword => fg.magenta,
            CodeStyle::Function => fg.blue,
            CodeStyle::Type => fg.cyan,
            CodeStyle::String => fg.green,
            CodeStyle::Number => fg.yellow,
            CodeStyle::Comment => theme.neutral_fg_secondary(),
        }
    }

    fn height_code_block_line(
        &self,
        node: &'ast AstNode<'ast>,
        node_code_block: &NodeCodeBlock,
        line: (Grapheme, Grapheme),
        synthetic: bool,
    ) -> f32 {
        let width = self.width(node) - 2. * self.layout.block_padding;
        let layout = self.layout_code_block_line(node, node_code_block, line, synthetic, false);
        self.compute_layout_from(layout, width, self.layout.row_height)
            .height
    }

    fn show_code_block_line(
        &mut self,
        ui: &mut Ui,
        node: &'ast AstNode<'ast>,
        top_left: Pos2,
        node_code_block: &NodeCodeBlock,
        line: (Grapheme, Grapheme),
        synthetic: bool,
    ) {
        let width = self.width(node) - 2. * self.layout.block_padding;
        let layout = self.layout_code_block_line(node, node_code_block, line, synthetic, true);
        let result = self.compute_layout_from(layout, width, self.layout.row_height);
        self.show_wrap_layout(ui, top_left, &result);
    }

    // "The closing code fence may be indented up to three spaces, and may be
    // followed only by spaces, which are ignored."
    // https://github.github.com/gfm/#fenced-code-blocks
}
