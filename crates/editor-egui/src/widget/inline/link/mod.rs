use comrak::Arena;
use comrak::nodes::{AstNode, NodeValue};
use editor_core::offset_types::{Grapheme, RangeExt as _};
use editor_core::operation_types::Operation;

use crate::TouchTarget;
use crate::input::{Event, Location, Region};
use crate::palette::ThemeExt as _;
use crate::resolvers::ResolvedLink;
use crate::widget::utils::wrap_layout::{Format, Layout};
use crate::{MdEdit, MdRender};

pub use crate::resolvers::LinkState;

impl<'ast> MdRender {
    pub fn text_format_link(&self, parent: &AstNode<'_>, state: LinkState) -> Format {
        let parent_text_format = self.text_format(parent);
        let theme = self.ctx.get_lb_theme();
        let color = match state {
            LinkState::Normal => theme.fg().blue,
            LinkState::Warning { .. } => theme.fg().yellow,
            LinkState::Broken { .. } => theme.fg().red,
        };
        Format {
            color,
            underline: true,
            ..parent_text_format
        }
    }

    fn link_is_auto(&self, node: &'ast AstNode<'ast>, url: &str) -> bool {
        self.infix_range(node)
            .is_some_and(|r| &self.buffer[r] == url)
    }

    /// Shared by producer + consumer so `ui.scope_id().with(salt)` resolves
    /// to the same id on both sides.
    pub fn link_interaction_id_salt(node_range: (Grapheme, Grapheme)) -> egui::Id {
        egui::Id::unique(("md_link", node_range))
    }

    /// Emit a link as a circumfix. A `Sense::click` interaction scope routes
    /// clicks here when they act on the link (cmd held, read-only, or
    /// unrevealed touch); otherwise a desktop link gets a hover-only scope
    /// and clicks fall through to the editor for cursor placement.
    pub fn layout_link(
        &self,
        layout: &mut Layout,
        node: &'ast AstNode<'ast>,
        range: (Grapheme, Grapheme),
    ) {
        let node_range = self.node_range(node);
        let url = node_link_url(node);
        let parent = node.parent().unwrap();
        let state = self.link_state_for_url(&url);
        let link_fmt = self.text_format_link(parent, state);
        // Interior-only: a cursor inside reveals raw markdown.
        let revealed = self.range_revealed_interior(node_range);

        // Cmd-click opens (desktop editor); read-only views have no cursor
        // to place, so a plain click opens; touch taps select the link —
        // unless revealed, when taps place the cursor in the raw source.
        let clickable = self.readonly
            || self.ctx.input(|i| i.modifiers.command)
            || (self.touch_mode && !revealed);
        let salt = Self::link_interaction_id_salt(node_range);
        let sense = self.link_sense(clickable);
        if let Some(sense) = sense {
            layout.interaction_open(salt, sense);
        }
        self.layout_circumfix(layout, node, range, link_fmt);
        if sense.is_some() {
            layout.interaction_close();
        }
    }

    /// A link's interaction scope: click when it opens on click, else hover
    /// on desktop so the pointer shows it can open. None on touch, where
    /// taps on an unopenable link place the cursor.
    pub fn link_sense(&self, clickable: bool) -> Option<egui::Sense> {
        if clickable {
            Some(egui::Sense::click())
        } else if !self.touch_mode {
            Some(egui::Sense::hover())
        } else {
            None
        }
    }

    pub fn resolve_link(&self, url: &str) -> Option<ResolvedLink> {
        self.link_resolver.resolve_link(url)
    }

    pub fn link_state_for_url(&self, url: &str) -> LinkState {
        self.link_resolver.link_state(url)
    }

    pub fn link_state_for_wikilink(&self, url: &str) -> LinkState {
        self.link_resolver.wikilink_state(url)
    }

    /// URL (or wikilink target) of the first link-like node whose source range
    /// intersects the current selection.
    pub fn selection_open_target(&mut self) -> Option<String> {
        let arena = Arena::new();
        let root = self.reparse(&arena);
        let selection = self.buffer.current.selection;
        for node in root.descendants() {
            let url = match &node.data.borrow().value {
                NodeValue::Link(l) | NodeValue::Image(l) => l.url.clone(),
                NodeValue::WikiLink(nwl) => nwl.url.clone(),
                _ => continue,
            };
            if !url.is_empty() && self.node_range(node).intersects(&selection, true) {
                return Some(url);
            }
        }
        None
    }

    /// Open every link and image in the current selection (desktop
    /// Cmd+Enter): one [`Event::OpenLink`] each, applied next frame.
    pub fn open_links_in_selection(&mut self, root: &'ast AstNode<'ast>) {
        let selection = self.buffer.current.selection;
        for node in root.descendants() {
            if !self.node_range(node).intersects(&selection, true) {
                continue;
            }
            let (url, wikilink) = match &node.data.borrow().value {
                NodeValue::WikiLink(nwl) => (nwl.url.clone(), true),
                NodeValue::Link(nl) | NodeValue::Image(nl) => (nl.url.clone(), false),
                _ => continue,
            };
            self.render_events.push(Event::OpenLink { url, wikilink });
        }
    }

    /// Hover → `PointingHand` when a click opens; click → open. The
    /// producer's interaction scope is the gate: cmd held (desktop),
    /// read-only, or touch — where the click instead selects the link
    /// ([`MdEdit::handle_link_menu_taps`]).
    pub fn handle_link_interactions(&mut self, root: &'ast AstNode<'ast>, ui: &egui::Ui) {
        let parent_base = ui.scope_id();
        for node in root.descendants() {
            let (url, is_wikilink) = match &node.data.borrow().value {
                NodeValue::WikiLink(nwl) => (nwl.url.clone(), true),
                NodeValue::Link(nl) | NodeValue::Image(nl) => (nl.url.clone(), false),
                _ => continue,
            };
            let id = parent_base.with(Self::link_interaction_id_salt(self.node_range(node)));

            let open = Event::OpenLink {
                url: url.clone(),
                wikilink: is_wikilink,
            };
            let tap = if self.readonly {
                open.clone()
            } else {
                Event::Select {
                    region: self.node_range(node).into(),
                }
            };
            self.touch_consume_interaction(id, TouchTarget::Tap(tap));

            let Some(response) = self.interaction_responses.get(&id) else {
                continue;
            };

            if response.hovered() && response.sense.senses_click() {
                ui.ctx()
                    .output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
            }

            if response.clicked() && (self.readonly || !self.touch_mode) {
                self.render_events.push(open);
                return;
            }
        }
    }
}

fn node_link_url(node: &AstNode<'_>) -> String {
    match &node.data.borrow().value {
        NodeValue::Link(link) | NodeValue::Image(link) => link.url.clone(),
        _ => String::new(),
    }
}

impl<'ast> MdEdit {
    /// The selection that edits a link-like node's hidden part: an image's
    /// or labeled link's destination, a wikilink's interior, a bare
    /// autolink's whole URL. `true` if acting on it should force-reveal via
    /// `entered_atom` — a bare autolink *is* its URL, so there is no
    /// interior sub-range whose selection would reveal the source.
    pub fn atom_edit_selection(&self, node: &'ast AstNode<'ast>) -> ((Grapheme, Grapheme), bool) {
        let node_range = self.renderer.node_range(node);
        let is_image = matches!(node.data.borrow().value, NodeValue::Image(_));
        match &node.data.borrow().value {
            NodeValue::WikiLink(_) => ((node_range.start() + 2, node_range.end() - 2), false),
            NodeValue::Link(_) | NodeValue::Image(_) => {
                let url = node_link_url(node);
                if !is_image && !url.is_empty() && self.renderer.link_is_auto(node, &url) {
                    (node_range, true)
                } else {
                    // `[title](url)` / `![alt](url)` — the postfix is
                    // `](url)`; without a label there are no children (no
                    // postfix) and the url starts after the opening syntax.
                    let open_len = if is_image { 4 } else { 3 };
                    let url_range = match self.renderer.postfix_range(node) {
                        Some(postfix) => (postfix.start() + 2, postfix.end() - 1),
                        None => (node_range.start() + open_len, node_range.end() - 1),
                    };
                    if url_range.0 <= url_range.1 {
                        (url_range, false)
                    } else {
                        // degenerate: interior caret
                        let caret = node_range.start() + (open_len - 2);
                        ((caret, caret), false)
                    }
                }
            }
            _ => (node_range, false),
        }
    }

    /// With the whole of a link selected, select the part that edits it
    /// (see [`Self::atom_edit_selection`]) — the touch path into a link.
    pub fn enter_at_link(
        &mut self,
        root: &'ast AstNode<'ast>,
        operations: &mut Vec<Operation>,
    ) -> bool {
        let selection = self.renderer.buffer.current.selection;
        for node in root.descendants() {
            if !matches!(
                node.data.borrow().value,
                NodeValue::Link(_) | NodeValue::WikiLink(_)
            ) {
                continue;
            }
            let node_range = self.renderer.node_range(node);
            if selection != node_range {
                continue;
            }
            let (select, force_reveal) = self.atom_edit_selection(node);
            if force_reveal {
                self.renderer.entered_atom = Some(node_range);
            }
            operations.push(Operation::Select(select));
            return true;
        }
        false
    }

    /// Touch tap on a plain rendered link or wikilink: select the link,
    /// mirroring embed taps ([`Self::handle_embed_tap`]). Applied via `ops`
    /// so the selection is current when the platform builds its menu.
    pub fn handle_link_menu_taps(
        &mut self,
        root: &'ast AstNode<'ast>,
        ui: &egui::Ui,
        id: egui::Id,
        ops: &mut Vec<Operation>,
    ) {
        if !self.renderer.touch_mode || self.renderer.readonly {
            return;
        }
        let parent_base = ui.scope_id();
        for node in root.descendants() {
            match &node.data.borrow().value {
                NodeValue::Link(_) | NodeValue::WikiLink(_) => {}
                _ => continue,
            }
            let node_range = self.renderer.node_range(node);
            let salt = MdRender::link_interaction_id_salt(node_range);
            let response = match self
                .renderer
                .interaction_responses
                .get(&parent_base.with(salt))
            {
                Some(r) => r.clone(), // clone to release the `renderer` borrow
                None => continue,
            };
            if response.clicked() {
                ui.memory_mut(|m| m.request_focus(id));
                let region = Region::BetweenLocations {
                    start: Location::Grapheme(node_range.start()),
                    end: Location::Grapheme(node_range.end()),
                };
                self.calc_operations(ui.ctx(), root, Event::Select { region }, ops);
                return;
            }
        }
    }
}
