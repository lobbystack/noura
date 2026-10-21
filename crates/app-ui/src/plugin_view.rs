//! A plugin page: the view its plugin drew, with the kit's widgets and the
//! theme's tokens, or the reason it can't be drawn in its place. Presses,
//! selections, header presses, menu choices, input changes and submits go
//! back to the plugin by node ID.
//!
//! What the widgets remember between frames, such as a list's selection or
//! the text you typed, lives in egui's memory under IDs made from the page
//! and the node ID, so a redraw with the same node IDs keeps it.

mod form;
mod menu;
mod rows;

use app_core::{App, PluginEvent, ViewKind, ViewNode};
use egui::{Frame, Id, Margin, RichText, Stroke, Ui, UiBuilder, accesskit::Role};

use crate::{
    common::muted,
    markdown,
    theme::{Palette, radius},
};

/// The page being drawn and what its widgets ask for.
struct Kit<'a> {
    page: &'a str,
    app: &'a mut App,
    /// What happened in the view this frame, oldest first.
    events: Vec<PluginEvent>,
    /// The link clicked in a Markdown node.
    link: Option<String>,
}

impl Kit<'_> {
    /// The ID of the widget that draws the node `node` of this page.
    fn id(&self, node: &str) -> Id {
        Id::unique(("plugin-node", self.page, node))
    }
}

pub(crate) fn show(ui: &mut Ui, app: &mut App, page_id: &str) {
    match app.plugin_view(page_id) {
        None => {
            muted(ui, "The plugin that adds this page isn't running.");
        }
        Some(Err(reason)) => problem(ui, &reason),
        Some(Ok(view)) => {
            let mut kit = Kit {
                page: page_id,
                app,
                events: Vec::new(),
                link: None,
            };
            ui.push_id(("plugin-view", page_id), |ui| {
                node(ui, view.root(), &mut kit);
            });
            let Kit { events, link, .. } = kit;
            for event in &events {
                app.send_plugin_event(page_id, event);
            }
            if let Some(url) = link {
                app.open_link(&url);
            }
        }
    }
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
        ViewKind::List { label } => rows::list(ui, node, label, kit),
        ViewKind::VirtualList { label, rows } => {
            rows::virtual_list(ui, node, label, *rows, kit);
        }
        ViewKind::Table { label, columns } => rows::table(ui, node, label, columns, kit),
        ViewKind::Form { label, submit } => form::form(ui, node, label, submit, kit),
        ViewKind::Input { .. } => form::input(ui, node, kit),
        // Their list, table or menu draws them.
        ViewKind::Item { .. }
        | ViewKind::Row { .. }
        | ViewKind::Menu { .. }
        | ViewKind::MenuItem { .. }
        | ViewKind::Separator => {}
    });
}
