use comrak::nodes::{AstNode, NodeCodeBlock};
use editor_core::offset_types::{Grapheme, RangeExt as _, RangeIterExt};
use egui::{Pos2, Rect, Stroke, Ui, Vec2};

use comrak::nodes::NodeValue;

use crate::MdRender;
use crate::widget::utils::consume_indent_columns;
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

    fn reveal_fenced_code_block(
        &self,
        node: &'ast AstNode<'ast>,
        node_code_block: &NodeCodeBlock,
    ) -> bool {
        let first_line_idx = self.node_first_line_idx(node);
        let last_line_idx = self.node_last_line_idx(node);

        if first_line_idx == last_line_idx {
            return true; // only opening fence: always reveal
        }
        if first_line_idx + 1 == last_line_idx
            && self.is_closing_fence(
                node,
                node_code_block,
                self.bounds.source_lines[last_line_idx],
            )
        {
            return true; // only opening + closing fence: always reveal
        }

        // Reveal when a reveal range touches any of the block's source
        // lines, *including* a container prefix (`> `): a cursor on the
        // prefix of an otherwise-collapsed fence line would otherwise have
        // no fragment, since the fence line isn't rendered.
        self.node_lines(node).iter().any(|line_idx| {
            let line = self.bounds.source_lines[line_idx];
            self.range_revealed(line, true)
        })
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

    fn reveal_indented_code_block(&self, node: &'ast AstNode<'ast>, synthetic: bool) -> bool {
        let mut reveal = false;
        for line_idx in self.node_lines(node).iter() {
            let line = self.bounds.source_lines[line_idx];
            let node_line = self.node_line(node, line);

            // reveal if selection is inside the indentation, but not at the
            // indentation's end / content's start
            let indentation = (
                node_line.start(),
                node_line
                    .end()
                    .min(node_line.start() + if synthetic { 0 } else { 4 }),
            );
            if self.range_revealed(indentation, false)
                || self
                    .reveal_ranges()
                    .any(|rr| rr.contains(indentation.start(), true, true))
            {
                reveal = true;
                break;
            }
        }
        reveal
    }

    /// Compute the `(chunk_start, end)` slice of `node_line` that's the
    /// actual content of a code-block line (fence indent stripped for
    /// fenced; 4-col indent stripped for indented, combined with parent
    /// item padding if any).
    fn code_line_range(
        &self,
        node: &'ast AstNode<'ast>,
        node_code_block: &NodeCodeBlock,
        node_line: (Grapheme, Grapheme),
        synthetic: bool,
    ) -> (Grapheme, Grapheme) {
        let NodeCodeBlock {
            fenced,
            fence_offset,
            ..
        } = node_code_block;
        if *fenced {
            // "If the leading code fence is indented N spaces, then up to N spaces
            // of indentation are removed from each line of the content..."
            // https://github.github.com/gfm/#fenced-code-blocks
            let text = &self.buffer[node_line];
            let indentation = text
                .chars()
                .take_while(|&c| c == ' ')
                .count()
                .min(*fence_offset);
            (node_line.start() + indentation, node_line.end())
        } else {
            // "An indented code block ... minus four spaces of indentation."
            // https://github.github.com/gfm/#indented-code-blocks
            //
            // Strip column-aware. If inside a list item, combine with
            // the item's padding so a tab straddling the boundary
            // doesn't get attributed entirely to one side.
            let target_cols = if synthetic {
                0
            } else {
                let parent_item_padding = node
                    .ancestors()
                    .find_map(|a| match &a.data.borrow().value {
                        NodeValue::Item(nl) => Some(nl.padding),
                        _ => None,
                    })
                    .unwrap_or(0);
                parent_item_padding + 4
            };
            let text = &self.buffer[node_line];
            let strip_graphemes = consume_indent_columns(text, target_cols);
            let chunk_start = (node_line.start() + strip_graphemes).min(node_line.end());
            (chunk_start, node_line.end())
        }
    }

    /// Build a `Layout` for one code-block line: the whole line goes in as
    /// one push_source with the code format. Syntax highlighting waits for
    /// a highlighter whose grammars have checked licenses.
    fn layout_code_block_line(
        &self,
        node: &'ast AstNode<'ast>,
        node_code_block: &NodeCodeBlock,
        line: (Grapheme, Grapheme),
        synthetic: bool,
    ) -> Layout {
        let node_line = self.node_line(node, line);
        let code_line = self.code_line_range(node, node_code_block, node_line, synthetic);
        let code_line_text = &self.buffer[code_line];
        let mut layout = Layout::new(code_line);
        if !code_line.is_empty() {
            layout.push_source(code_line, code_line_text, self.text_format(node));
        }
        layout
    }

    fn height_code_block_line(
        &self,
        node: &'ast AstNode<'ast>,
        node_code_block: &NodeCodeBlock,
        line: (Grapheme, Grapheme),
        synthetic: bool,
    ) -> f32 {
        let width = self.width(node) - 2. * self.layout.block_padding;
        let layout = self.layout_code_block_line(node, node_code_block, line, synthetic);
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
        let layout = self.layout_code_block_line(node, node_code_block, line, synthetic);
        let result = self.compute_layout_from(layout, width, self.layout.row_height);
        self.show_wrap_layout(ui, top_left, &result);
    }

    // "The closing code fence may be indented up to three spaces, and may be
    // followed only by spaces, which are ignored."
    // https://github.github.com/gfm/#fenced-code-blocks

    fn is_closing_fence(
        &self,
        node: &'ast AstNode<'ast>,
        node_code_block: &NodeCodeBlock,
        line: (Grapheme, Grapheme),
    ) -> bool {
        let NodeCodeBlock {
            fence_char,
            fence_length,
            ..
        } = node_code_block;
        let fence_char = *fence_char as char;

        let node_line = self.node_line(node, line);

        let text = &self.buffer[node_line];
        let mut chars = text.chars().peekable();

        // Skip up to 3 leading spaces
        for _ in 0..3 {
            if chars.peek() == Some(&' ') {
                chars.next();
            } else {
                break;
            }
        }

        // Skip exactly fence_length fence_char's
        for _ in 0..*fence_length {
            if chars.peek() == Some(&fence_char) {
                chars.next();
            } else {
                return false;
            }
        }

        // Skip any number of additional fence_char's
        loop {
            if chars.peek() == Some(&fence_char) {
                chars.next();
            } else {
                break;
            }
        }

        // Any additional characters may be spaces only
        chars.all(|c| c == ' ')
    }
}
