//! The Ignored files page: the workspace's patterns, with a field to add
//! one and a button to remove each. The workspace switcher opens it in a
//! dialog; Settings can host the same page later.

use app_core::App;
use egui::{Button, Key, RichText, TextEdit, Ui, UiBuilder, accesskit::Role};

use crate::{common::muted, theme::Palette};

/// What the page remembers between frames.
#[derive(Default)]
pub(crate) struct IgnoredFilesView {
    /// The pattern being typed.
    pattern: String,
    /// The pattern field took keyboard focus when the page opened.
    focused: bool,
}

/// Draw the page's contents for the open Ignored files list.
pub(crate) fn page(ui: &mut Ui, app: &mut App, view: &mut IgnoredFilesView) {
    let Some(list) = app.ignored_files().cloned() else {
        return;
    };
    let palette = Palette::of(ui.ctx());
    muted(
        ui,
        "Noura leaves files and folders that match these patterns out of the file tree and \
         search. Patterns work like a .gitignore file.",
    );
    ui.add_space(8.0);
    let mut removed = None;
    if list.patterns.is_empty() {
        muted(ui, "No patterns yet.");
    }
    for pattern in &list.patterns {
        ui.horizontal(|ui| {
            ui.label(RichText::new(pattern).monospace());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let remove = ui
                    .add(Button::new("\u{d7}").frame(false))
                    .accessible_name(format!("Stop ignoring {pattern}"))
                    .on_hover_text("Stop ignoring");
                if remove.clicked() {
                    removed = Some(pattern.clone());
                }
            });
        });
    }
    ui.add_space(8.0);
    let mut add = false;
    ui.horizontal(|ui| {
        let field = ui
            .add(
                TextEdit::singleline(&mut view.pattern)
                    .hint_text("drafts/")
                    .desired_width(ui.available_width() - 56.0),
            )
            .accessible_name("Pattern to ignore");
        if !view.focused {
            view.focused = true;
            field.request_focus();
        }
        add |= field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
        add |= ui
            .add_enabled(!view.pattern.trim().is_empty(), Button::new("Add"))
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
