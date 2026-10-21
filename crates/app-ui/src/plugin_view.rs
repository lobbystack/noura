//! A plugin page: the view its plugin drew, with the kit's widgets and the
//! theme's tokens, or the reason it can't be drawn in its place. Presses go
//! back to the plugin by node ID.

use app_core::{App, ViewKind, ViewNode};
use egui::{Frame, Margin, RichText, Stroke, Ui, UiBuilder, accesskit::Role};

use crate::{
    common::muted,
    markdown,
    theme::{Palette, radius},
};

/// What a press or a click in the view asks for.
#[derive(Default)]
struct Asked {
    /// The button pressed, by node ID.
    pressed: Option<String>,
    /// The link clicked in a Markdown node.
    link: Option<String>,
}

pub(crate) fn show(ui: &mut Ui, app: &mut App, page_id: &str) {
    match app.plugin_view(page_id) {
        None => {
            muted(ui, "The plugin that adds this page isn't running.");
        }
        Some(Err(reason)) => problem(ui, &reason),
        Some(Ok(view)) => {
            let mut asked = Asked::default();
            ui.push_id(("plugin-view", page_id), |ui| {
                node(ui, view.root(), &mut asked);
            });
            if let Some(node) = asked.pressed {
                app.press_plugin_button(page_id, &node);
            }
            if let Some(url) = asked.link {
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
                    RichText::new("This page can't be shown")
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
