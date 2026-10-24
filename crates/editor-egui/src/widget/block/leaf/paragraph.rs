use comrak::nodes::AstNode;
use editor_core::offset_types::{Grapheme, RangeExt, RangeIterExt as _};
use editor_core::resolvers::EmbeddedNote;
use editor_core::spans::{InlineSpan, SpanKind};
use egui::{Pos2, Ui};

use crate::MdRender;
use crate::palette::ThemeExt as _;
use crate::widget::utils::wrap_layout::{Format, Layout, WrapUnitLayout};

impl<'ast> MdRender {
    pub fn height_paragraph(&self, node: &'ast AstNode<'ast>) -> f32 {
        let mut result = 0.;
        let last_line_idx = self.node_last_line_idx(node);
        for line_idx in self.node_lines(node).iter() {
            let line = self.bounds.source_lines[line_idx];
            let node_line = self.node_line(node, line);

            result += self.height_paragraph_line(node, node_line);

            if line_idx != last_line_idx {
                result += self.layout.block_spacing;
            }
        }

        result
    }

    /// Build a `Layout` for one paragraph line. Shared by height +
    /// show passes — both consume the resulting `WrapUnitLayout`.
    /// Walks the paragraph's pre/infix/post slices (handles leading/
    /// trailing whitespace per CommonMark "raw content" rule), and
    /// recurses into inline children for the inline range.
    fn layout_paragraph_line(
        &self,
        node: &'ast AstNode<'ast>,
        node_line: (Grapheme, Grapheme),
    ) -> Layout {
        let mut layout = Layout::new(node_line);
        if let Some((pre_node, pre_children, _, post_children, post_node)) =
            self.split_range(node, node_line)
        {
            let fmt = self.text_format(node);
            if !pre_node.is_empty() {
                layout.push_source(pre_node, &self.buffer[pre_node], fmt.clone());
            }
            if !pre_children.is_empty() {
                layout.push_source(pre_children, &self.buffer[pre_children], fmt.clone());
            }
            self.layout_inline_children(&mut layout, node, node_line);
            if !post_children.is_empty() {
                layout.push_source(post_children, &self.buffer[post_children], fmt.clone());
            }
            if !post_node.is_empty() {
                layout.push_source(post_node, &self.buffer[post_node], fmt);
            }
        } else {
            // Empty paragraph line such as in `- [ ] \n  x`.
            let fmt = self.text_format(node);
            layout.push_source(node_line, &self.buffer[node_line], fmt);
        }
        layout
    }

    /// The note embed a paragraph line draws as a block: an `![[note]]`
    /// alone on the line, away from the caret, whose note the embedding
    /// app found.
    fn block_embed(&self, node_line: (Grapheme, Grapheme)) -> Option<(InlineSpan, EmbeddedNote)> {
        self.spans_in(node_line).find_map(|span| {
            if !matches!(span.kind, SpanKind::Embed { alone: true, .. })
                || !node_line.contains_range(&span.range, true, true)
                || self.span_revealed(span)
            {
                return None;
            }
            self.embedded_note(span).map(|note| (span.clone(), note))
        })
    }

    /// The title and the text an embed block draws, each as wide as the
    /// block's inside.
    fn embed_block_layouts(
        &self,
        node: &'ast AstNode<'ast>,
        span: &InlineSpan,
        note: &EmbeddedNote,
    ) -> (WrapUnitLayout, WrapUnitLayout) {
        let width = self.width(node) - 2.0 * self.layout.block_padding;
        let row_height = self.layout.row_height;
        let format = self.text_format(node);
        let title = self.compute_override_section_layout_new(
            span.range,
            &note.title,
            width,
            row_height,
            Format {
                bold: true,
                ..format.clone()
            },
        );
        let end = span.range.end();
        let text = self.compute_override_section_layout_new(
            (end, end),
            &note.text,
            width,
            row_height,
            Format {
                color: self.ctx.get_lb_theme().neutral_fg_secondary(),
                ..format
            },
        );
        (title, text)
    }

    fn height_embed_block(
        &self,
        node: &'ast AstNode<'ast>,
        span: &InlineSpan,
        note: &EmbeddedNote,
    ) -> f32 {
        let (title, text) = self.embed_block_layouts(node, span, note);
        2.0 * self.layout.block_padding + title.height + self.layout.row_spacing + text.height
    }

    fn show_embed_block(
        &mut self,
        ui: &mut Ui,
        node: &'ast AstNode<'ast>,
        top_left: Pos2,
        span: &InlineSpan,
        note: &EmbeddedNote,
    ) {
        let height = self.height_embed_block(node, span, note);
        let rect = egui::Rect::from_min_size(top_left, egui::vec2(self.width(node), height));
        let theme = self.ctx.get_lb_theme();
        ui.painter()
            .rect_filled(rect, 4.0, theme.neutral_bg_secondary().gamma_multiply(0.5));
        ui.painter().rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(1.0, theme.neutral_bg_tertiary()),
            egui::StrokeKind::Inside,
        );
        let (title, text) = self.embed_block_layouts(node, span, note);
        let inside = top_left + egui::Vec2::splat(self.layout.block_padding);
        self.show_wrap_layout(ui, inside, &title);
        let below = inside + egui::vec2(0.0, title.height + self.layout.row_spacing);
        self.show_wrap_layout(ui, below, &text);
    }

    pub fn height_paragraph_line(
        &self,
        node: &'ast AstNode<'ast>,
        node_line: (Grapheme, Grapheme),
    ) -> f32 {
        if let Some((span, note)) = self.block_embed(node_line) {
            return self.height_embed_block(node, &span, &note);
        }
        let width = self.width(node);
        let layout = self.layout_paragraph_line(node, node_line);
        self.compute_layout_from(layout, width, self.layout.row_height)
            .height
    }

    pub fn show_paragraph(&mut self, ui: &mut Ui, node: &'ast AstNode<'ast>, mut top_left: Pos2) {
        for line in self.node_lines(node).iter() {
            let line = self.bounds.source_lines[line];
            let node_line = self.node_line(node, line);

            let line_height = self.height_paragraph_line(node, node_line);

            self.show_paragraph_line(ui, node, top_left, node_line);
            self.show_block_line_prefixes(ui, node, line, top_left, self.layout.row_height);
            top_left.y += line_height;

            top_left.y += self.layout.block_spacing;
        }
    }

    pub fn show_paragraph_line(
        &mut self,
        ui: &mut Ui,
        node: &'ast AstNode<'ast>,
        top_left: Pos2,
        node_line: (Grapheme, Grapheme),
    ) {
        if let Some((span, note)) = self.block_embed(node_line) {
            self.show_embed_block(ui, node, top_left, &span, &note);
            return;
        }
        let width = self.width(node);
        let layout = self.layout_paragraph_line(node, node_line);
        let result = self.compute_layout_from(layout, width, self.layout.row_height);
        self.show_wrap_layout(ui, top_left, &result);
    }
}
