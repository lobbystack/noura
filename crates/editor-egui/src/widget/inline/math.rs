use comrak::nodes::AstNode;
use editor_core::offset_types::{Grapheme, IntoRangeExt as _, RangeExt as _};

use crate::MdRender;
use crate::palette::ThemeExt as _;
use crate::widget::utils::wrap_layout::{Format, Layout};

impl<'ast> MdRender {
    /// Math draws as its source, in monospace italics: the text stays as
    /// written and only the dollar signs hide away from the caret.
    pub fn text_format_math(&self, parent: &AstNode<'_>) -> Format {
        Format {
            italic: true,
            color: self.ctx.get_lb_theme().fg().magenta,
            ..self.text_format_code_block(parent)
        }
    }

    pub fn layout_math(
        &self,
        layout: &mut Layout,
        node: &'ast AstNode<'ast>,
        range: (Grapheme, Grapheme),
    ) {
        let node_range = self.node_range(node);
        // Display math spans lines, and each line lays out on its own.
        if node_range.trim(&range).is_empty() {
            return;
        }
        let [open, close] = self.math_delimiters(node);
        let reveal = self.node_revealed(node);
        let format = self.text_format_math(node.parent().unwrap());
        let source = (open.end(), close.start().max(open.end())).trim(&range);
        // Each part trimmed to this line, in source order.
        let delimiter = |layout: &mut Layout, part: (Grapheme, Grapheme), anchor: Grapheme| {
            let part = part.trim(&range);
            if part.is_empty() {
                return;
            }
            if reveal {
                layout.push_source(part, &self.buffer[part], self.text_format_syntax());
            } else {
                layout.push_override(anchor.into_range(), "", self.text_format_syntax());
            }
        };
        delimiter(layout, open, open.start());
        if !source.is_empty() {
            layout.push_source(source, &self.buffer[source], format);
        }
        delimiter(layout, close, close.end());
    }
}
