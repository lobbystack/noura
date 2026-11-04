//! The Keyboard shortcuts page: every shortcut by area, from `app-core`'s
//! command registry and the keys each part of the window handles itself.
//! Each area lists its shortcuts as rows with dividers, the keys in caps
//! on the right.

use app_core::App;
use egui::{Align, Color32, Layout, RichText, Sense, Stroke, Ui, UiBuilder, accesskit::Role, vec2};

use crate::{
    keys,
    theme::{BODY_SIZE, Palette, SUPPORTING_SIZE, Weight, font, radius},
};

/// Draw the page's groups in `ui`.
pub(crate) fn page(ui: &mut Ui, app: &App) {
    let platform = app.platform();
    let palette = Palette::of(ui.ctx());
    ui.spacing_mut().item_spacing.y = 0.0;
    ui.spacing_mut().interact_size.y = 20.0;
    for (index, group) in app.shortcut_groups().into_iter().enumerate() {
        let title = group.area.title();
        if index > 0 {
            ui.add_space(32.0);
        }
        ui.label(
            RichText::new(title)
                .size(SUPPORTING_SIZE)
                .color(palette.muted_foreground),
        );
        ui.add_space(4.0);
        let builder = UiBuilder::new()
            .accessibility_role(Role::List)
            .accessibility_label(title);
        ui.scope_builder(builder, |ui| {
            let count = group.entries.len();
            for (row_index, entry) in group.entries.iter().enumerate() {
                let keys = keys::shortcuts_text(&entry.shortcuts, platform);
                // Each row reads as its action, with its keys as the
                // description.
                let row = UiBuilder::new()
                    .accessibility_role(Role::ListItem)
                    .accessibility_label(&entry.label);
                let response = ui
                    .scope_builder(row, |ui| {
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            ui.set_min_height(20.0);
                            ui.label(RichText::new(&entry.label).size(BODY_SIZE));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                for (index, shortcut) in entry.shortcuts.iter().enumerate().rev() {
                                    for key in keys::shortcut_keys(shortcut, platform).iter().rev()
                                    {
                                        key_cap(ui, key, &palette);
                                    }
                                    if index > 0 {
                                        ui.label(
                                            RichText::new("or")
                                                .size(SUPPORTING_SIZE)
                                                .color(palette.muted_foreground),
                                        );
                                    }
                                }
                            });
                        });
                        ui.add_space(12.0);
                    })
                    .response;
                if row_index + 1 < count {
                    ui.painter().hline(
                        response.rect.x_range(),
                        response.rect.bottom(),
                        Stroke::new(1.0, palette.border),
                    );
                }
                ui.ctx().accesskit_node_builder(response.id, |node| {
                    node.set_description(keys.clone());
                    node.set_keyboard_shortcut(keys);
                });
            }
        });
    }
}

/// One key in the legacy key cap: 20 px high, rounded, on the muted fill.
fn key_cap(ui: &mut Ui, key: &str, palette: &Palette) {
    let font = font(ui.ctx(), SUPPORTING_SIZE, Weight::Medium);
    let galley = ui
        .painter()
        .layout_no_wrap(key.to_owned(), font, Color32::PLACEHOLDER);
    let width = (galley.size().x + 8.0).max(20.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 20.0), Sense::hover());
    ui.painter().rect_filled(rect, radius(1.0), palette.muted);
    ui.painter().galley(
        rect.center() - galley.size() / 2.0,
        galley,
        palette.muted_foreground,
    );
}
