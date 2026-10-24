use comrak::nodes::AstNode;
use editor_core::offset_types::Grapheme;

use crate::MdRender;
use crate::widget::utils::wrap_layout::Layout;

impl<'ast> MdRender {
    pub fn layout_wikilink(
        &self,
        layout: &mut Layout,
        node: &'ast AstNode<'ast>,
        range: (Grapheme, Grapheme),
    ) {
        let url = match &node.data.borrow().value {
            comrak::nodes::NodeValue::WikiLink(nwl) => nwl.url.clone(),
            _ => String::new(),
        };
        let parent = node.parent().unwrap();
        let node_range = self.node_range(node);
        let state = self.link_state_for_wikilink(&url);
        let fmt = self.text_format_link(parent, state);
        let revealed = self.range_revealed_interior(node_range);
        let clickable = self.link_clickable(revealed);
        let salt = Self::link_interaction_id_salt(node_range);
        let sense = self.link_sense(clickable);
        if let Some(sense) = sense {
            layout.interaction_open(salt, sense);
        }
        self.layout_circumfix(layout, node, range, fmt);
        if sense.is_some() {
            layout.interaction_close();
        }
    }
}
