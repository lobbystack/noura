//! The Ignored files page: the workspace's patterns, with a field to add
//! one and a button to remove each. Settings' Ignored files section
//! draws it.

use app_core::App;
use egui::{Frame, Key, Margin, RichText, Stroke, TextEdit, Ui, UiBuilder, accesskit::Role};

use crate::{
    common::{Btn, Size},
    icons,
    theme::{BODY_SIZE, Palette, pill},
};

/// What the page remembers between frames.
#[derive(Default)]
pub(crate) struct IgnoredFilesView {
    /// The pattern being typed.
    pattern: String,
    /// The pattern field took keyboard focus when the page opened.
    focused: bool,
}

/// Draw the page's contents for the open Ignored files list: each
/// pattern in a bordered row with its remove button, then the field that
/// adds one.
pub(crate) fn page(ui: &mut Ui, app: &mut App, view: &mut IgnoredFilesView) {
    let Some(list) = app.ignored_files().cloned() else {
        return;
    };
    let palette = Palette::of(ui.ctx());
    let mut removed = None;
    if !list.patterns.is_empty() {
        ui.spacing_mut().item_spacing.y = 10.0;
        for pattern in &list.patterns {
            Frame::new()
                .stroke(Stroke::new(1.0, palette.border))
                .corner_radius(pill())
                .inner_margin(Margin::symmetric(14, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(pattern).monospace().size(BODY_SIZE));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let label = format!("Stop ignoring {pattern}");
                            let remove = Btn::icon_only(icons::CLOSE, &label)
                                .size(Size::Sm)
                                .show(ui)
                                .on_hover_text("Stop ignoring");
                            if remove.clicked() {
                                removed = Some(pattern.clone());
                            }
                        });
                    });
                });
        }
        ui.add_space(6.0);
    }
    let mut add = false;
    ui.horizontal(|ui| {
        let field = ui
            .add(
                TextEdit::singleline(&mut view.pattern)
                    .hint_text("drafts/")
                    .desired_width(ui.available_width() - 64.0),
            )
            .accessible_name("Pattern to ignore");
        if !view.focused {
            view.focused = true;
            field.request_focus();
        }
        add |= field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
        add |= Btn::new("Add")
            .enabled(!view.pattern.trim().is_empty())
            .show(ui)
            .clicked();
    });
    if let Some(problem) = &list.problem {
        let builder = UiBuilder::new().accessibility_role(Role::Alert);
        ui.scope_builder(builder, |ui| {
            ui.label(RichText::new(problem).color(palette.destructive));
        });
    }
    if add {
        if app.add_ignored(&view.pattern) {
            view.pattern.clear();
        }
        // Keep typing patterns without reaching for the field.
        view.focused = false;
    }
    if let Some(pattern) = removed {
        app.remove_ignored(&pattern);
    }
}
