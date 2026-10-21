//! The Keyboard shortcuts page: every shortcut by area, from `app-core`'s
//! command registry and the keys each part of the window handles itself.

use app_core::App;
use egui::{Align, Layout, RichText, ScrollArea, Ui, UiBuilder, accesskit::Role};

use crate::{
    keys,
    theme::{Palette, SUPPORTING_SIZE},
};

/// Draw the page's groups in `ui`.
pub(crate) fn page(ui: &mut Ui, app: &App) {
    let platform = app.platform();
    let theme = Palette::of(ui.ctx());
    ScrollArea::vertical()
        .id_salt("keyboard-shortcuts")
        .max_height(440.0)
        .show(ui, |ui| {
            for group in app.shortcut_groups() {
                let title = group.area.title();
                ui.add_space(8.0);
                ui.label(
                    RichText::new(title)
                        .size(SUPPORTING_SIZE)
                        .color(theme.muted_foreground),
                );
                let builder = UiBuilder::new()
                    .accessibility_role(Role::List)
                    .accessibility_label(title);
                ui.scope_builder(builder, |ui| {
                    for entry in &group.entries {
                        let keys = keys::shortcuts_text(&entry.shortcuts, platform);
                        // Each row reads as its action, with its keys as
                        // the description.
                        let row = UiBuilder::new()
                            .accessibility_role(Role::ListItem)
                            .accessibility_label(&entry.label);
                        let response = ui
                            .scope_builder(row, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(&entry.label);
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        ui.label(
                                            RichText::new(&keys).color(theme.muted_foreground),
                                        );
                                    });
                                });
                            })
                            .response;
                        ui.ctx().accesskit_node_builder(response.id, |node| {
                            node.set_description(keys.clone());
                            node.set_keyboard_shortcut(keys);
                        });
                    }
                });
            }
        });
}
