use comrak::nodes::{AstNode, NodeValue};
use editor_core::offset_types::{Grapheme, RangeExt as _};
use editor_core::operation_types::Operation;

use crate::TouchTarget;
use crate::input::{Event, Location, Region};
use crate::palette::ThemeExt as _;
use crate::widget::utils::wrap_layout::{Format, Layout};
use crate::{MdEdit, MdRender};
use editor_core::links::node_link_url;

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

    /// Shared by producer + consumer so `ui.scope_id().with(salt)` resolves
    /// to the same id on both sides.
    pub fn link_interaction_id_salt(node_range: (Grapheme, Grapheme)) -> egui::Id {
        egui::Id::unique(("md_link", node_range))
    }

    /// Emit a link as a circumfix. A `Sense::click` interaction scope routes
    /// clicks here when they act on the link (a drawn link, cmd held, or
    /// read-only); otherwise a desktop link's visible source gets a
    /// hover-only scope and clicks fall through to the editor for cursor
    /// placement.
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

        let clickable = self.link_clickable(revealed);
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

    /// Whether a click acts on a link rather than placing the caret in it.
    /// A click on a drawn link opens it, and so does a Cmd-click (Ctrl-click
    /// off Apple platforms) on its visible source, while a plain click there
    /// places the caret. Read-only views have no caret to place, so a plain
    /// click opens. Touch taps select a drawn link, and place the caret in
    /// visible source.
    pub fn link_clickable(&self, revealed: bool) -> bool {
        self.readonly || !revealed || self.ctx.input(|i| i.modifiers.command)
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

    /// Hover → `PointingHand` when a click opens; click → open, in a new
    /// tab with cmd held; middle-click → open in a new tab, on drawn links
    /// and visible source alike. The producer's interaction scope is the
    /// gate for clicks: a drawn link, cmd held (desktop) or read-only, and
    /// on touch the click instead selects the link
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

            let open = |new_tab| Event::OpenLink {
                url: url.clone(),
                wikilink: is_wikilink,
                new_tab,
            };
            let tap = if self.readonly {
                open(false)
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

            // Visible source senses only hover, so egui gives the middle
            // click to the editor under it and stops reporting the source
            // hovered while the button is down; the pointer is still on it.
            let middle_clicked = !self.touch_mode
                && response.contains_pointer()
                && ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Middle));
            if middle_clicked {
                self.render_events.push(open(true));
                return;
            }
            if response.clicked() && (self.readonly || !self.touch_mode) {
                let new_tab = !self.touch_mode && ui.input(|i| i.modifiers.command);
                self.render_events.push(open(new_tab));
                return;
            }
        }
    }
}

impl<'ast> MdEdit {
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
                self.request_focus(ui.ctx(), id);
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
