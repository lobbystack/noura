use comrak::nodes::AstNode;
use editor_core::nodes::is_single_tilde_strikethrough;
use editor_core::offset_types::{Grapheme, RangeExt as _};

use crate::MdRender;
use crate::widget::utils::wrap_layout::{Format, Layout};

impl<'ast> MdRender {
    pub fn text_format_strikethrough(&self, parent: &AstNode<'_>) -> Format {
        let parent_text_format = self.text_format(parent);
        Format {
            strikethrough: true,
            ..parent_text_format
        }
    }

    pub fn layout_strikethrough(
        &self,
        layout: &mut Layout,
        node: &'ast AstNode<'ast>,
        range: (Grapheme, Grapheme),
    ) {
        if is_single_tilde_strikethrough(node) {
            // `H~2~O` draws as written: its tildes are text.
            let format = self.text_format(node.parent().unwrap());
            let push = |part: Option<(Grapheme, Grapheme)>, layout: &mut Layout| {
                if let Some(part) = part.map(|part| part.trim(&range))
                    && !part.is_empty()
                {
                    layout.push_source(part, &self.buffer[part], format.clone());
                }
            };
            push(self.prefix_range(node), layout);
            self.layout_inline_children(layout, node, range);
            push(self.postfix_range(node), layout);
            return;
        }
        let fmt = self.text_format_strikethrough(node.parent().unwrap());
        self.layout_circumfix(layout, node, range, fmt);
    }
}
