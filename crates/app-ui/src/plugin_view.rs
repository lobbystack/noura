//! A plugin page or properties panel: the view its plugin drew, with the
//! kit's widgets and the theme's tokens, or the reason it can't be drawn
//! in its place. Presses, selections, header presses, menu choices, input
//! changes, submits and drops go back to the plugin by node ID.
//!
//! What the widgets remember between frames, such as a list's selection,
//! an open menu or the text you typed, lives in egui's memory under IDs
//! made from the window, the page or panel, and the node ID. A redraw with
//! the same node IDs keeps it, and each window that shows the view keeps
//! its own: every window draws with one egui context and shares its
//! memory.

mod board;
mod calendar;
mod drag;
mod form;
mod menu;
mod rows;

use std::ops::Range;

use app_core::{App, PluginEvent, PluginView, TabId, ViewKind, ViewNode, VirtualRow};
use egui::{Frame, Id, Margin, RichText, Stroke, Ui, UiBuilder, accesskit::Role};

use crate::{
    common::muted,
    markdown,
    theme::{Palette, radius},
};

/// Where a plugin's view shows.
#[derive(Debug, Clone, Copy, Hash)]
enum Place<'a> {
    /// The rail page with this ID.
    Page(&'a str),
    /// The properties panel of the object open in this tab.
    Panel(TabId),
}

/// The view being drawn and what its widgets ask for.
struct Kit<'a> {
    /// The window's salt for IDs.
    window: Id,
    place: Place<'a>,
    app: &'a mut App,
    /// What happened in the view this frame, oldest first.
    events: Vec<PluginEvent>,
    /// The link clicked in a Markdown node.
    link: Option<String>,
}

impl Kit<'_> {
    /// The ID of the widget that draws the node `node` of this view in
    /// this window.
    fn id(&self, node: &str) -> Id {
        Id::unique(("plugin-node", self.window, self.place, node))
    }

    /// The rows in `range` of the virtual list `list`, each `None` until
    /// the plugin returns it.
    fn rows(&mut self, list: &str, range: Range<u32>) -> Result<Vec<Option<VirtualRow>>, String> {
        match self.place {
            Place::Page(page) => self.app.plugin_rows(page, list, range),
            // App-core refuses a panel that holds a virtual list.
            Place::Panel(_) => Err(format!(
                "The virtual list {list} can't show in a properties panel."
            )),
        }
    }
}

/// The plugin page `page_id` in the window whose ID salt is `window`.
pub(crate) fn show(ui: &mut Ui, app: &mut App, window: Id, page_id: &str) {
    let Some(view) = app.plugin_view(page_id) else {
        muted(ui, "The plugin that adds this page isn't running.");
        return;
    };
    let (events, link) = draw(ui, app, window, Place::Page(page_id), &view);
    for event in &events {
        app.send_plugin_event(page_id, event);
    }
    if let Some(url) = link {
        app.open_link(&url);
    }
}

/// The properties panel a plugin draws beside the body of an object of its
/// own type, in the note screen's panel slot of the window whose ID salt
/// is `window`. Events go back to the plugin by node ID.
pub(crate) fn show_panel(ui: &mut Ui, app: &mut App, window: Id, tab: TabId, view: &PluginView) {
    let (events, link) = draw(ui, app, window, Place::Panel(tab), view);
    for event in &events {
        app.send_panel_event(tab, event);
    }
    if let Some(url) = link {
        app.open_link(&url);
    }
}

/// Draw `view` at `place`, or the reason it can't be drawn, and return
/// what happened in it this frame, oldest first, and the link clicked.
fn draw(
    ui: &mut Ui,
    app: &mut App,
    window: Id,
    place: Place,
    view: &PluginView,
) -> (Vec<PluginEvent>, Option<String>) {
    let view = match view {
        Err(reason) => {
            problem(ui, reason);
            return (Vec::new(), None);
        }
        Ok(view) => view,
    };
    let mut kit = Kit {
        window,
        place,
        app,
        events: Vec::new(),
        link: None,
    };
    ui.push_id(("plugin-view", window, place), |ui| {
        node(ui, view.root(), &mut kit);
    });
    (kit.events, kit.link)
}

/// The reason a view can't be drawn, where the view would be.
fn problem(ui: &mut Ui, reason: &str) {
    let palette = Palette::of(ui.ctx());
    let builder = UiBuilder::new().accessibility_role(Role::Alert);
    ui.scope_builder(builder, |ui| {
        Frame::new()
            .stroke(Stroke::new(1.0, palette.border))
            .corner_radius(radius(1.0))
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new("Noura can't show this page")
                        .strong()
                        .color(palette.destructive),
                );
                ui.label(reason);
            });
    });
}

/// A titled box, as panels and forms draw.
fn boxed(ui: &mut Ui, role: Role, label: Option<&str>, add: impl FnOnce(&mut Ui)) {
    let mut builder = UiBuilder::new().accessibility_role(role);
    if let Some(label) = label {
        builder = builder.accessibility_label(label);
    }
    let palette = Palette::of(ui.ctx());
    ui.scope_builder(builder, |ui| {
        Frame::new()
            .stroke(Stroke::new(1.0, palette.border))
            .corner_radius(radius(1.0))
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.vertical(add);
            });
    });
}

fn node(ui: &mut Ui, node: &ViewNode, kit: &mut Kit) {
    ui.push_id(&node.id, |ui| match &node.kind {
        ViewKind::Panel { label } => {
            boxed(ui, Role::Group, label.as_deref(), |ui| {
                for child in &node.children {
                    self::node(ui, child, kit);
                }
            });
        }
        ViewKind::Text(text) => {
            ui.label(text);
        }
        ViewKind::Markdown(text) => {
            if let Some(url) = markdown::show(ui, text) {
                kit.link = Some(url);
            }
        }
        ViewKind::Button { label } => menu::button(ui, node, label, kit),
        ViewKind::List { label, reorder } => rows::list(ui, node, label, *reorder, kit),
        ViewKind::Board { label } => board::board(ui, node, label, kit),
        ViewKind::Calendar { label, mode, date } => {
            calendar::calendar(ui, node, label, *mode, date, kit);
        }
        ViewKind::VirtualList { label, rows } => {
            rows::virtual_list(ui, node, label, *rows, kit);
        }
        ViewKind::Table { label, columns } => rows::table(ui, node, label, columns, kit),
        ViewKind::Form { label, submit } => form::form(ui, node, label, submit, kit),
        ViewKind::Input { .. } => form::input(ui, node, kit),
        // Their list, table, board or menu draws them.
        ViewKind::Item { .. }
        | ViewKind::Row { .. }
        | ViewKind::BoardColumn { .. }
        | ViewKind::Card { .. }
        | ViewKind::Entry { .. }
        | ViewKind::Menu { .. }
        | ViewKind::MenuItem { .. }
        | ViewKind::Separator => {}
    });
}
