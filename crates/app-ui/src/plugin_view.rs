//! Plugin views: a plugin page, an Inbox section, a plugin's page in
//! Settings or a properties panel, drawn from the nodes the plugin
//! returned with the kit's widgets and the theme's tokens, or the reason
//! it can't be drawn in its place. Presses go back to the plugin by node
//! ID.

use app_core::{App, PluginView, TabId, ViewKind, ViewNode};
use egui::{Frame, Margin, RichText, Stroke, Ui, UiBuilder, accesskit::Role};

use crate::{
    common::muted,
    markdown,
    theme::{Palette, radius},
};

/// What a press or a click in a view asks for.
#[derive(Default)]
pub(crate) struct Asked {
    /// The button pressed, by node ID.
    pub(crate) pressed: Option<String>,
    /// The link clicked in a Markdown node.
    pub(crate) link: Option<String>,
}

/// The plugin page `page_id`.
pub(crate) fn show(ui: &mut Ui, app: &mut App, page_id: &str) {
    let Some(view) = app.plugin_view(page_id) else {
        muted(ui, "The plugin that adds this page isn't running.");
        return;
    };
    let asked = draw(ui, ("plugin-view", page_id), &view);
    if let Some(node) = asked.pressed {
        app.press_plugin_button(page_id, &node);
    }
    if let Some(url) = asked.link {
        app.open_link(&url);
    }
}

/// The properties panel a plugin draws beside the body of an object of its
/// own type, in the note screen's panel slot. Presses go back to the
/// plugin by node ID.
pub(crate) fn show_panel(ui: &mut Ui, app: &mut App, tab: TabId, view: &PluginView) {
    let asked = draw(ui, ("plugin-panel", tab), view);
    if let Some(node) = asked.pressed {
        app.press_panel_button(tab, &node);
    }
    if let Some(url) = asked.link {
        app.open_link(&url);
    }
}

/// Draw `view`, or why it can't be drawn, under the ID `salt`, and say
/// what it asked for.
pub(crate) fn draw(
    ui: &mut Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    view: &PluginView,
) -> Asked {
    let mut asked = Asked::default();
    match view {
        Err(reason) => problem(ui, reason),
        Ok(view) => {
            ui.push_id(salt, |ui| {
                node(ui, view.root(), &mut asked);
            });
        }
    }
    asked
}

/// The reason a view can't be drawn, where the view would be.
fn problem(ui: &mut Ui, reason: &str) {
    let palette = Palette::of(ui.ctx());
    let builder = UiBuilder::new()
        .accessibility_role(Role::Alert)
        .accessibility_label(reason);
    ui.scope_builder(builder, |ui| {
        Frame::new()
            .stroke(Stroke::new(1.0, palette.border))
            .corner_radius(radius(1.0))
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new("Noura can't show this")
                        .strong()
                        .color(palette.destructive),
                );
                ui.label(reason);
            });
    });
}

fn node(ui: &mut Ui, node: &ViewNode, asked: &mut Asked) {
    ui.push_id(&node.id, |ui| match &node.kind {
        ViewKind::Panel { label } => {
            let mut builder = UiBuilder::new().accessibility_role(Role::Group);
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
                        ui.vertical(|ui| {
                            for child in &node.children {
                                self::node(ui, child, asked);
                            }
                        });
                    });
            });
        }
        ViewKind::Text(text) => {
            ui.label(text);
        }
        ViewKind::Markdown(text) => {
            if let Some(url) = markdown::show(ui, text) {
                asked.link = Some(url);
            }
        }
        ViewKind::Button { label } => {
            if ui.button(label).clicked() {
                asked.pressed = Some(node.id.clone());
            }
        }
    });
}
